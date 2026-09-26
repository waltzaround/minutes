//! LLM selection: catalog models plus custom GGUF imports (advanced users).
//!
//! Custom models have unknown architecture, so their memory needs are
//! estimated conservatively from file size (and a large KV-cache allowance).

use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::models::catalog::{self, LlmSpec};
use crate::models::manager::ModelManager;
use crate::storage::{now, Database};
use crate::system::memory_budget::{MemoryFootprint, KIB, MIB};

pub const CUSTOM_PREFIX: &str = "custom-";

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LlmChoice {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub custom: bool,
    pub byte_size: u64,
}

pub fn custom_models(db: &Database) -> Vec<(String, PathBuf, u64)> {
    db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT model_id, path, byte_size FROM model_installations WHERE source = 'custom_import' AND status = 'installed'",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get::<_, i64>(2)? as u64)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
    .unwrap_or_default()
}

/// Path to the GGUF for an LLM id, if installed.
pub fn model_path(db: &Database, models: &ModelManager, id: &str) -> Option<PathBuf> {
    if let Some(dir) = models.installed_dir(id) {
        let m = catalog::manifest_by_id(id)?;
        return Some(dir.join(&m.files[0].path));
    }
    custom_models(db).into_iter().find(|(mid, p, _)| mid == id && p.exists()).map(|(_, p, _)| p)
}

/// Runtime characteristics for budgeting. Custom models get a conservative
/// estimate.
pub fn spec_for(db: &Database, id: &str) -> Option<LlmSpec> {
    if let Some(s) = catalog::llm_spec(id) {
        return Some(s.clone());
    }
    let (_, _, size) = custom_models(db).into_iter().find(|(mid, _, _)| mid == id)?;
    Some(LlmSpec {
        id: id.to_string(),
        footprint: MemoryFootprint { weights_bytes: size, kv_bytes_per_token: 128 * KIB, fixed_overhead_bytes: 768 * MIB },
        total_params_billions: 1.0,
        active_params_billions: 1.0,
        mixture_of_experts: false,
        max_context: 32_768,
    })
}

pub fn choices(db: &Database, models: &ModelManager) -> Vec<LlmChoice> {
    let mut out: Vec<LlmChoice> = catalog::manifests()
        .iter()
        .filter(|m| m.purpose == catalog::ModelPurpose::Llm)
        .map(|m| LlmChoice {
            id: m.id.clone(),
            name: m.name.clone(),
            installed: models.installed_dir(&m.id).is_some(),
            custom: false,
            byte_size: m.byte_size,
        })
        .collect();
    for (id, path, size) in custom_models(db) {
        out.push(LlmChoice {
            name: path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| id.clone()),
            id,
            installed: path.exists(),
            custom: true,
            byte_size: size,
        });
    }
    out
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-").chars().take(60).collect()
}

/// Copy a user-provided GGUF into the models directory and register it.
pub fn import_gguf(db: &Database, models_dir: &Path, source: &Path) -> anyhow::Result<LlmChoice> {
    let mut f = std::fs::File::open(source)?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    anyhow::ensure!(&magic == b"GGUF", "This file is not a GGUF model.");
    let file_name = source.file_name().ok_or_else(|| anyhow::anyhow!("invalid file name"))?.to_string_lossy().into_owned();
    let id = format!("{CUSTOM_PREFIX}{}", slug(file_name.trim_end_matches(".gguf")));
    let size = std::fs::metadata(source)?.len();
    let free = crate::system::capabilities::detect_disk(models_dir).available_bytes;
    anyhow::ensure!(free == 0 || free > size + crate::models::download::WORKING_SPACE_RESERVE, "Not enough free disk space to import this model.");
    let dir = models_dir.join(&id);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(&file_name);
    let tmp = dir.join(format!("{file_name}.part"));
    std::fs::copy(source, &tmp)?;
    let mut hasher = Sha256::new();
    let mut r = std::fs::File::open(&tmp)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    std::fs::rename(&tmp, &dest)?;
    let digest = hex::encode(hasher.finalize());
    db.with(|c| {
        c.execute(
            "INSERT INTO model_installations (model_id, version, purpose, path, byte_size, bytes_downloaded, status, source, error, installed_at, updated_at)
             VALUES (?1, ?2, 'llm', ?3, ?4, ?4, 'installed', 'custom_import', NULL, ?5, ?5)
             ON CONFLICT(model_id) DO UPDATE SET path = excluded.path, byte_size = excluded.byte_size, version = excluded.version, updated_at = excluded.updated_at",
            params![id, format!("sha256:{digest}"), dest.display().to_string(), size as i64, now()],
        )
    })?;
    Ok(LlmChoice { id, name: file_name, installed: true, custom: true, byte_size: size })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_validates_and_registers() {
        let db = Database::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("x.gguf");
        std::fs::write(&bad, b"nope").unwrap();
        assert!(import_gguf(&db, &dir.path().join("models"), &bad).is_err());

        let good = dir.path().join("My Model Q4.gguf");
        std::fs::write(&good, b"GGUF\x03\x00\x00\x00rest").unwrap();
        let c = import_gguf(&db, &dir.path().join("models"), &good).unwrap();
        assert_eq!(c.id, "custom-my-model-q4");
        let models = ModelManager::new(db.clone(), dir.path().join("models"));
        assert!(model_path(&db, &models, &c.id).unwrap().exists());
        let spec = spec_for(&db, &c.id).unwrap();
        assert_eq!(spec.footprint.weights_bytes, good.metadata().unwrap().len());
        assert!(choices(&db, &models).iter().any(|x| x.id == c.id && x.custom));
    }
}
