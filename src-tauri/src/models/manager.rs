//! Installed-model state: which catalog models are present and verified on
//! disk, where they live, and removal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::catalog::{self, ModelManifest};
use crate::storage::{now, Database};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InstallState {
    NotInstalled,
    Downloading,
    Paused,
    Verifying,
    Installed,
    Failed,
}

impl InstallState {
    fn from_db(s: &str) -> Self {
        match s {
            "downloading" => InstallState::Downloading,
            "paused" => InstallState::Paused,
            "verifying" => InstallState::Verifying,
            "installed" => InstallState::Installed,
            "failed" => InstallState::Failed,
            _ => InstallState::NotInstalled,
        }
    }

    pub fn as_db(self) -> &'static str {
        match self {
            InstallState::Downloading => "downloading",
            InstallState::Paused => "paused",
            InstallState::Verifying => "verifying",
            InstallState::Installed => "installed",
            InstallState::Failed => "failed",
            InstallState::NotInstalled => "not_installed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelStatus {
    pub manifest: ModelManifest,
    pub state: InstallState,
    pub bytes_downloaded: u64,
    pub path: Option<String>,
    pub error: Option<String>,
}

/// Live progress for downloads in flight (not persisted on every chunk).
#[derive(Debug, Clone, Default)]
pub struct LiveProgress {
    pub bytes_downloaded: u64,
    pub state: Option<InstallState>,
}

pub struct ModelManager {
    db: Database,
    models_dir: RwLock<PathBuf>,
    live: RwLock<HashMap<String, LiveProgress>>,
}

impl ModelManager {
    pub fn new(db: Database, models_dir: PathBuf) -> Self {
        ModelManager { db, models_dir: RwLock::new(models_dir), live: RwLock::new(HashMap::new()) }
    }

    pub fn models_dir(&self) -> PathBuf {
        self.models_dir.read().clone()
    }

    pub fn set_models_dir(&self, dir: PathBuf) {
        *self.models_dir.write() = dir;
    }

    pub fn model_dir(&self, id: &str) -> PathBuf {
        self.models_dir().join(id)
    }

    pub fn set_live(&self, id: &str, progress: LiveProgress) {
        self.live.write().insert(id.to_string(), progress);
    }

    pub fn clear_live(&self, id: &str) {
        self.live.write().remove(id);
    }

    /// All files present with the expected size. SHA-256 is verified at
    /// install time; re-hashing gigabytes on every launch is not worth it.
    pub fn files_present(&self, m: &ModelManifest) -> bool {
        let dir = self.model_dir(&m.id);
        m.files.iter().all(|f| std::fs::metadata(dir.join(&f.path)).is_ok_and(|md| md.len() == f.byte_size))
    }

    /// Install directory if the model is installed and intact.
    pub fn installed_dir(&self, id: &str) -> Option<PathBuf> {
        let m = catalog::manifest_by_id(id)?;
        let state = self.db_state(id).map(|(s, _, _)| s);
        (state == Some(InstallState::Installed) && self.files_present(m)).then(|| self.model_dir(id))
    }

    fn db_state(&self, id: &str) -> Option<(InstallState, u64, Option<String>)> {
        self.db
            .with(|c| {
                c.query_row(
                    "SELECT status, bytes_downloaded, error FROM model_installations WHERE model_id = ?1",
                    [id],
                    |r| Ok((InstallState::from_db(&r.get::<_, String>(0)?), r.get::<_, i64>(1)? as u64, r.get(2)?)),
                )
                .optional()
            })
            .ok()
            .flatten()
    }

    pub fn status(&self, m: &ModelManifest) -> ModelStatus {
        let live = self.live.read().get(&m.id).cloned();
        let (mut state, mut bytes, error) = self.db_state(&m.id).unwrap_or((InstallState::NotInstalled, 0, None));
        if state == InstallState::Installed && !self.files_present(m) {
            // Files were removed or moved outside the app.
            state = InstallState::NotInstalled;
            bytes = 0;
        }
        // A "downloading" row without a live task is left over from a
        // crash or quit: treat it as paused (resumable).
        if state == InstallState::Downloading && live.is_none() {
            state = InstallState::Paused;
        }
        if let Some(l) = live {
            bytes = l.bytes_downloaded;
            if let Some(s) = l.state {
                state = s;
            }
        }
        ModelStatus {
            manifest: m.clone(),
            state,
            bytes_downloaded: bytes,
            path: (state == InstallState::Installed).then(|| self.model_dir(&m.id).display().to_string()),
            error,
        }
    }

    pub fn list(&self) -> Vec<ModelStatus> {
        catalog::manifests().iter().map(|m| self.status(m)).collect()
    }

    pub fn record(&self, m: &ModelManifest, state: InstallState, bytes: u64, error: Option<&str>) -> rusqlite::Result<()> {
        let path = self.model_dir(&m.id).display().to_string();
        let ts = now();
        let installed_at = (state == InstallState::Installed).then(|| ts.clone());
        self.db.with(|c| {
            c.execute(
                "INSERT INTO model_installations
                   (model_id, version, purpose, path, byte_size, bytes_downloaded, status, source, error, installed_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'catalog', ?8, ?9, ?10)
                 ON CONFLICT(model_id) DO UPDATE SET
                   version = excluded.version, path = excluded.path, byte_size = excluded.byte_size,
                   bytes_downloaded = excluded.bytes_downloaded, status = excluded.status, error = excluded.error,
                   installed_at = COALESCE(excluded.installed_at, model_installations.installed_at),
                   updated_at = excluded.updated_at",
                params![
                    m.id,
                    m.version,
                    m.purpose.as_db(),
                    path,
                    m.byte_size as i64,
                    bytes as i64,
                    state.as_db(),
                    error,
                    installed_at,
                    ts
                ],
            )
            .map(|_| ())
        })
    }

    /// Delete a model's files and its install record.
    pub fn remove(&self, id: &str) -> anyhow::Result<()> {
        let dir = self.model_dir(id);
        if dir.exists() {
            ensure_inside(&self.models_dir(), &dir)?;
            std::fs::remove_dir_all(&dir)?;
        }
        self.db.with(|c| c.execute("DELETE FROM model_installations WHERE model_id = ?1", [id]))?;
        self.clear_live(id);
        Ok(())
    }
}

/// Guard against deleting anything outside the models directory.
fn ensure_inside(root: &Path, path: &Path) -> anyhow::Result<()> {
    let root = std::fs::canonicalize(root)?;
    let path = std::fs::canonicalize(path)?;
    anyhow::ensure!(path.starts_with(&root) && path != root, "refusing to delete outside the models directory");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::catalog::VAD_ID;

    #[test]
    fn stale_downloading_row_is_reported_paused() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(Database::open_in_memory().unwrap(), dir.path().to_path_buf());
        let m = catalog::manifest_by_id(VAD_ID).unwrap();
        mgr.record(m, InstallState::Downloading, 100, None).unwrap();
        assert_eq!(mgr.status(m).state, InstallState::Paused);
    }

    #[test]
    fn installed_requires_files_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(Database::open_in_memory().unwrap(), dir.path().to_path_buf());
        let m = catalog::manifest_by_id(VAD_ID).unwrap();
        mgr.record(m, InstallState::Installed, m.byte_size, None).unwrap();
        assert_eq!(mgr.status(m).state, InstallState::NotInstalled);
        assert!(mgr.installed_dir(VAD_ID).is_none());

        let mdir = mgr.model_dir(VAD_ID);
        std::fs::create_dir_all(&mdir).unwrap();
        let f = std::fs::File::create(mdir.join(&m.files[0].path)).unwrap();
        f.set_len(m.files[0].byte_size).unwrap();
        assert_eq!(mgr.status(m).state, InstallState::Installed);
        assert!(mgr.installed_dir(VAD_ID).is_some());

        mgr.remove(VAD_ID).unwrap();
        assert!(!mdir.exists());
        assert_eq!(mgr.status(m).state, InstallState::NotInstalled);
    }
}
