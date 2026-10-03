//! Resumable, verified model downloads.
//!
//! - Only started by an explicit user action (never silently).
//! - Disk space is checked up front: remaining bytes + a working-space
//!   reserve for meeting audio and temporary files, so we never fill the disk.
//! - Each file downloads to `<name>.part`, resumes with HTTP Range requests,
//!   is verified with SHA-256, then atomically renamed into place.
//! - Pause keeps partial files; cancel deletes them. A crash leaves `.part`
//!   files that the next attempt resumes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use ts_rs::TS;

use super::catalog::{ModelFile, ModelManifest};
use super::manager::{InstallState, LiveProgress, ModelManager};
use crate::system::memory_budget::GIB;

/// Space kept free after a download for meeting audio and temp files.
pub const WORKING_SPACE_RESERVE: u64 = 3 * GIB;
/// Consecutive attempts that make *no* progress before giving up. Attempts
/// that downloaded something reset the count, so long downloads survive
/// any number of CDN stalls or dropped connections.
const MAX_STALLED_ATTEMPTS: u32 = 6;
/// No bytes for this long counts as a stalled connection.
const STALL_TIMEOUT: Duration = Duration::from_secs(45);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const PERSIST_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("Not enough free disk space. {required} bytes are needed but only {available} are free.")]
    InsufficientDisk { required: u64, available: u64 },
    #[error("The download could not be completed: {0}")]
    Network(String),
    #[error("The downloaded file {0} did not match its expected checksum and was discarded.")]
    Integrity(String),
    #[error("Download paused")]
    Paused,
    #[error("Download cancelled")]
    Cancelled,
    #[error("This model is already downloading")]
    AlreadyRunning,
    #[error("File error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DownloadProgress {
    pub model_id: String,
    pub file: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub state: InstallState,
    pub error: Option<String>,
}

const PAUSE: u8 = 1;
const CANCEL: u8 = 2;

#[derive(Default)]
struct Control(AtomicU8);

impl Control {
    fn check(&self) -> Result<(), DownloadError> {
        match self.0.load(Ordering::Relaxed) {
            PAUSE => Err(DownloadError::Paused),
            CANCEL => Err(DownloadError::Cancelled),
            _ => Ok(()),
        }
    }
}

pub struct Downloader {
    client: reqwest::Client,
    manager: Arc<ModelManager>,
    active: Mutex<HashMap<String, Arc<Control>>>,
}

/// Bytes needed on disk to finish downloading `remaining` bytes safely.
pub fn required_free_space(remaining: u64) -> u64 {
    remaining + WORKING_SPACE_RESERVE + (remaining / 20).max(GIB / 2)
}

impl Downloader {
    pub fn new(manager: Arc<ModelManager>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Minutes/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(STALL_TIMEOUT)
            // Hugging Face's CDN stalls long HTTP/2 streams; HTTP/1.1 is
            // measurably steadier for multi-gigabyte files.
            .http1_only()
            .build()
            .expect("http client");
        Downloader { client, manager, active: Mutex::new(HashMap::new()) }
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.lock().contains_key(id)
    }

    pub fn pause(&self, id: &str) {
        if let Some(c) = self.active.lock().get(id) {
            c.0.store(PAUSE, Ordering::Relaxed);
        }
    }

    /// Cancel a running download, or discard a paused/failed one.
    pub fn cancel(&self, m: &ModelManifest) -> anyhow::Result<()> {
        if let Some(c) = self.active.lock().get(&m.id) {
            c.0.store(CANCEL, Ordering::Relaxed);
            return Ok(()); // the task cleans up
        }
        discard_partials(&self.manager.model_dir(&m.id), m)?;
        if self.manager.status(m).state != InstallState::Installed {
            self.manager.remove(&m.id)?;
        }
        Ok(())
    }

    /// Bytes already on disk for this model (complete files + partials).
    fn bytes_on_disk(&self, m: &ModelManifest) -> u64 {
        let dir = self.manager.model_dir(&m.id);
        m.files
            .iter()
            .map(|f| {
                let done = std::fs::metadata(dir.join(&f.path)).map(|md| md.len()).unwrap_or(0);
                if done == f.byte_size {
                    done
                } else {
                    std::fs::metadata(part_path(&dir, f)).map(|md| md.len().min(f.byte_size)).unwrap_or(0)
                }
            })
            .sum()
    }

    pub fn check_disk(&self, m: &ModelManifest) -> Result<(), DownloadError> {
        let dir = self.manager.models_dir();
        std::fs::create_dir_all(&dir)?;
        let remaining = m.byte_size.saturating_sub(self.bytes_on_disk(m));
        let available = crate::system::capabilities::detect_disk(&dir).available_bytes;
        let required = required_free_space(remaining);
        if available > 0 && available < required {
            return Err(DownloadError::InsufficientDisk { required, available });
        }
        Ok(())
    }

    /// Download (or resume) every file of `m`. Emits progress through
    /// `on_progress`. Returns when installed, paused, cancelled or failed.
    pub async fn download(
        &self,
        m: &ModelManifest,
        on_progress: impl Fn(DownloadProgress) + Send + Sync,
    ) -> Result<(), DownloadError> {
        let control = {
            let mut active = self.active.lock();
            if active.contains_key(&m.id) {
                return Err(DownloadError::AlreadyRunning);
            }
            let c = Arc::new(Control::default());
            active.insert(m.id.clone(), c.clone());
            c
        };
        let result = self.download_inner(m, &control, &on_progress).await;
        self.active.lock().remove(&m.id);

        let bytes = self.bytes_on_disk(m);
        let (state, err) = match &result {
            Ok(()) => (InstallState::Installed, None),
            Err(DownloadError::Paused) => (InstallState::Paused, None),
            Err(DownloadError::Cancelled) => {
                let _ = discard_partials(&self.manager.model_dir(&m.id), m);
                let _ = self.manager.remove(&m.id);
                (InstallState::NotInstalled, None)
            }
            Err(e) => (InstallState::Failed, Some(e.to_string())),
        };
        if state != InstallState::NotInstalled {
            if let Err(e) = self.manager.record(m, state, bytes, err.as_deref()) {
                tracing::error!(error = %e, model = m.id, "failed to record model state");
            }
        }
        self.manager.clear_live(&m.id);
        on_progress(DownloadProgress {
            model_id: m.id.clone(),
            file: String::new(),
            bytes_downloaded: bytes,
            total_bytes: m.byte_size,
            state,
            error: err,
        });
        result
    }

    async fn download_inner(
        &self,
        m: &ModelManifest,
        control: &Control,
        on_progress: &(impl Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), DownloadError> {
        self.check_disk(m)?;
        let dir = self.manager.model_dir(&m.id);
        tokio::fs::create_dir_all(&dir).await?;
        self.manager
            .record(m, InstallState::Downloading, self.bytes_on_disk(m), None)
            .map_err(|e| DownloadError::Io(std::io::Error::other(e)))?;

        let mut completed_before: u64 = 0;
        for f in &m.files {
            let final_path = dir.join(&f.path);
            if std::fs::metadata(&final_path).is_ok_and(|md| md.len() == f.byte_size) {
                completed_before += f.byte_size;
                continue;
            }
            // Manifests can contain nested files (e.g. test_wavs/en.wav).
            // Their .part files live alongside the final file, so create
            // the parent on every attempt, including retries of old failures.
            if let Some(parent) = final_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // Re-hashing what's already on disk can take a while for large
            // files; show that instead of looking stuck.
            let partial = tokio::fs::metadata(part_path(&dir, f)).await.map(|md| md.len()).unwrap_or(0);
            if partial > 0 {
                self.manager.set_live(&m.id, LiveProgress { bytes_downloaded: completed_before + partial, state: Some(InstallState::Verifying) });
                on_progress(DownloadProgress {
                    model_id: m.id.clone(),
                    file: f.path.clone(),
                    bytes_downloaded: completed_before + partial,
                    total_bytes: m.byte_size,
                    state: InstallState::Verifying,
                    error: None,
                });
            }
            let mut state = FileState::resume(&part_path(&dir, f), f.byte_size).await?;
            let mut stalled = 0u32;
            loop {
                let before = state.offset;
                match self.download_file(m, f, &dir, completed_before, &mut state, control, on_progress).await {
                    Ok(()) => break,
                    Err(DownloadError::Network(msg)) => {
                        stalled = if state.offset > before { 1 } else { stalled + 1 };
                        if stalled > MAX_STALLED_ATTEMPTS {
                            return Err(DownloadError::Network(msg));
                        }
                        tracing::warn!(model = m.id, file = f.path, stalled, offset = state.offset, error = msg, "download interrupted; resuming");
                        let _ = self.manager.record(m, InstallState::Downloading, completed_before + state.offset, None);
                        tokio::time::sleep(backoff(stalled)).await;
                        control.check()?;
                    }
                    Err(e) => return Err(e),
                }
            }
            completed_before += f.byte_size;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_file(
        &self,
        m: &ModelManifest,
        f: &ModelFile,
        dir: &Path,
        completed_before: u64,
        state: &mut FileState,
        control: &Control,
        on_progress: &(impl Fn(DownloadProgress) + Send + Sync),
    ) -> Result<(), DownloadError> {
        let part = part_path(dir, f);
        if state.offset < f.byte_size {
            let mut req = self.client.get(&f.download_url);
            if state.offset > 0 {
                req = req.header(reqwest::header::RANGE, format!("bytes={}-", state.offset));
            }
            let resp = req.send().await.map_err(network)?;
            let status = resp.status();
            match status.as_u16() {
                206 => {}
                200 => state.restart(), // server ignored the range: start this file again
                _ => return Err(DownloadError::Network(format!("server responded with {status}"))),
            }
            // Make the file length match exactly what has been hashed.
            let file = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part).await?;
            file.set_len(state.offset).await?;
            let mut file = tokio::io::BufWriter::with_capacity(1 << 20, file);
            file.seek(std::io::SeekFrom::Start(state.offset)).await?;

            let mut stream = resp.bytes_stream();
            let mut last_emit = Instant::now() - PROGRESS_INTERVAL;
            let mut last_persist = Instant::now();
            let result: Result<(), DownloadError> = async {
                while let Some(chunk) = stream.next().await {
                    control.check()?;
                    let chunk = chunk.map_err(network)?;
                    if state.offset + chunk.len() as u64 > f.byte_size {
                        return Err(DownloadError::Integrity(f.path.clone()));
                    }
                    file.write_all(&chunk).await?;
                    state.hasher.update(&chunk);
                    state.offset += chunk.len() as u64;
                    let total_done = completed_before + state.offset;
                    self.manager.set_live(&m.id, LiveProgress { bytes_downloaded: total_done, state: Some(InstallState::Downloading) });
                    if last_emit.elapsed() >= PROGRESS_INTERVAL {
                        last_emit = Instant::now();
                        on_progress(DownloadProgress {
                            model_id: m.id.clone(),
                            file: f.path.clone(),
                            bytes_downloaded: total_done,
                            total_bytes: m.byte_size,
                            state: InstallState::Downloading,
                            error: None,
                        });
                    }
                    if last_persist.elapsed() >= PERSIST_INTERVAL {
                        last_persist = Instant::now();
                        let _ = self.manager.record(m, InstallState::Downloading, total_done, None);
                    }
                }
                Ok(())
            }
            .await;
            // Always flush what was hashed, even when the stream failed.
            file.flush().await?;
            file.get_ref().sync_all().await?;
            result?;
        }

        if state.offset != f.byte_size {
            return Err(DownloadError::Network("connection closed before the file finished".into()));
        }
        self.manager.set_live(&m.id, LiveProgress { bytes_downloaded: completed_before + state.offset, state: Some(InstallState::Verifying) });
        let digest = hex::encode(state.hasher.clone().finalize());
        if !digest.eq_ignore_ascii_case(&f.sha256) {
            let _ = tokio::fs::remove_file(&part).await;
            state.restart();
            tracing::error!(model = m.id, file = f.path, expected = f.sha256, actual = digest, "checksum mismatch");
            return Err(DownloadError::Integrity(f.path.clone()));
        }
        tokio::fs::rename(&part, dir.join(&f.path)).await?;
        Ok(())
    }
}

fn backoff(stalled: u32) -> Duration {
    if cfg!(test) {
        return Duration::from_millis(10);
    }
    Duration::from_secs((1u64 << stalled.min(5)).min(30))
}

/// Bytes of a file already on disk and their running SHA-256. Kept across
/// retries so a resumed download never re-reads what it already verified.
struct FileState {
    offset: u64,
    hasher: Sha256,
}

impl FileState {
    async fn resume(part: &Path, size: u64) -> Result<Self, DownloadError> {
        let len = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        if len == 0 || len > size {
            return Ok(FileState { offset: 0, hasher: Sha256::new() });
        }
        Ok(FileState { offset: len, hasher: hash_prefix(part).await? })
    }

    fn restart(&mut self) {
        self.offset = 0;
        self.hasher = Sha256::new();
    }
}

/// Network error with its full cause chain (reqwest's top-level message
/// alone, e.g. "error decoding response body", hides timeouts and resets).
fn network(e: reqwest::Error) -> DownloadError {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    DownloadError::Network(msg)
}

fn part_path(dir: &Path, f: &ModelFile) -> PathBuf {
    dir.join(format!("{}.part", f.path))
}

async fn hash_prefix(path: &Path) -> Result<Sha256, DownloadError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<Sha256> {
        use std::io::Read;
        let mut hasher = Sha256::new();
        let mut file = std::fs::File::open(path)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher)
    })
    .await
    .map_err(|e| DownloadError::Io(std::io::Error::other(e)))?
    .map_err(DownloadError::Io)
}

fn discard_partials(dir: &Path, m: &ModelManifest) -> std::io::Result<()> {
    for f in &m.files {
        let p = part_path(dir, f);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::catalog::ModelPurpose;
    use crate::storage::Database;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal HTTP/1.1 server with Range support. `fail_after` truncates the
    /// first response to simulate a dropped connection.
    async fn serve(body: Vec<u8>, fail_after: Option<usize>) -> (String, Arc<AtomicUsize>) {
        serve_with(body, fail_after, false).await
    }

    async fn serve_with(body: Vec<u8>, fail_after: Option<usize>, cut_every: bool) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let body = body.clone();
                let n = hits2.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let len = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..len]).to_lowercase();
                    let start = req
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim_end_matches('-').trim_end_matches("-\r").split('-').next()?.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let slice = &body[start.min(body.len())..];
                    let status = if start > 0 { "206 Partial Content" } else { "200 OK" };
                    let head = format!(
                        "HTTP/1.1 {status}\r\ncontent-length: {}\r\ncontent-range: bytes {start}-{}/{}\r\nconnection: close\r\n\r\n",
                        slice.len(),
                        body.len().saturating_sub(1),
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let send = match fail_after {
                        Some(cut) if n == 0 || cut_every => &slice[..cut.min(slice.len())],
                        _ => slice,
                    };
                    let _ = sock.write_all(send).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (format!("http://{addr}/file.bin"), hits)
    }

    fn manifest(url: &str, body: &[u8]) -> ModelManifest {
        ModelManifest {
            id: "test-model".into(),
            name: "Test".into(),
            purpose: ModelPurpose::Vad,
            version: "1".into(),
            byte_size: body.len() as u64,
            files: vec![ModelFile {
                path: "file.bin".into(),
                download_url: url.into(),
                byte_size: body.len() as u64,
                sha256: hex::encode(Sha256::digest(body)),
            }],
            license: "test".into(),
            license_url: "https://example.invalid".into(),
            minimum_capabilities: None,
        }
    }

    fn setup() -> (tempfile::TempDir, Arc<ModelManager>, Downloader) {
        let dir = tempfile::tempdir().unwrap();
        let mgr = Arc::new(ModelManager::new(Database::open_in_memory().unwrap(), dir.path().join("models")));
        let dl = Downloader::new(mgr.clone());
        (dir, mgr, dl)
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let (url, _) = serve(body.clone(), None).await;
        let (_d, mgr, dl) = setup();
        let m = manifest(&url, &body);
        dl.download(&m, |_| {}).await.unwrap();
        assert_eq!(std::fs::read(mgr.model_dir(&m.id).join("file.bin")).unwrap(), body);
        assert!(!mgr.model_dir(&m.id).join("file.bin.part").exists());
    }

    #[tokio::test]
    async fn downloads_nested_file_on_fresh_install() {
        let body = vec![3u8; 10_000];
        let (url, _) = serve(body.clone(), None).await;
        let (_d, mgr, dl) = setup();
        let mut m = manifest(&url, &body);
        m.files[0].path = "test_wavs/en.wav".into();

        dl.download(&m, |_| {}).await.unwrap();

        assert_eq!(std::fs::read(mgr.model_dir(&m.id).join(&m.files[0].path)).unwrap(), body);
        assert!(!part_path(&mgr.model_dir(&m.id), &m.files[0]).exists());
        assert_eq!(mgr.status(&m).state, InstallState::Installed);
    }

    #[tokio::test]
    async fn retries_failed_install_with_missing_nested_directory() {
        let body = vec![4u8; 10_000];
        let (url, hits) = serve(body.clone(), None).await;
        let (_d, mgr, dl) = setup();
        let mut m = manifest(&url, &body);
        let mut nested = m.files[0].clone();
        nested.path = "test_wavs/en.wav".into();
        m.files.push(nested);
        m.byte_size *= 2;
        let dir = mgr.model_dir(&m.id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.bin"), &body).unwrap();
        mgr.record(&m, InstallState::Failed, body.len() as u64, Some("File error: missing directory")).unwrap();

        dl.download(&m, |_| {}).await.unwrap();

        assert_eq!(hits.load(Ordering::SeqCst), 1, "completed files should be reused");
        assert_eq!(std::fs::read(dir.join("test_wavs/en.wav")).unwrap(), body);
        assert_eq!(mgr.status(&m).state, InstallState::Installed);
        assert!(mgr.status(&m).error.is_none());
    }

    #[tokio::test]
    async fn resumes_after_dropped_connection() {
        let body: Vec<u8> = (0..500_000u32).map(|i| (i * 7 % 256) as u8).collect();
        let (url, hits) = serve(body.clone(), Some(120_000)).await;
        let (_d, mgr, dl) = setup();
        let m = manifest(&url, &body);
        dl.download(&m, |_| {}).await.unwrap();
        assert!(hits.load(Ordering::SeqCst) >= 2, "second request should resume");
        assert_eq!(std::fs::read(mgr.model_dir(&m.id).join("file.bin")).unwrap(), body);
    }

    #[tokio::test]
    async fn survives_many_interruptions_while_progressing() {
        // Every response is cut after 40 kB: far more interruptions than the
        // stall limit, but each one makes progress, so the download completes.
        let body: Vec<u8> = (0..600_000u32).map(|i| (i * 13 % 256) as u8).collect();
        let (url, hits) = serve_with(body.clone(), Some(40_000), true).await;
        let (_d, mgr, dl) = setup();
        let m = manifest(&url, &body);
        dl.download(&m, |_| {}).await.unwrap();
        assert!(hits.load(Ordering::SeqCst) as u32 > MAX_STALLED_ATTEMPTS + 1);
        assert_eq!(std::fs::read(mgr.model_dir(&m.id).join("file.bin")).unwrap(), body);
    }

    #[tokio::test]
    async fn gives_up_when_no_progress_is_made() {
        let body = vec![7u8; 100_000];
        let (url, hits) = serve_with(body.clone(), Some(0), true).await;
        let (_d, _mgr, dl) = setup();
        let m = manifest(&url, &body);
        let err = dl.download(&m, |_| {}).await.unwrap_err();
        assert!(matches!(err, DownloadError::Network(_)));
        assert_eq!(hits.load(Ordering::SeqCst) as u32, MAX_STALLED_ATTEMPTS + 1);
    }

    #[tokio::test]
    async fn checksum_mismatch_is_rejected() {
        let body = vec![1u8; 10_000];
        let (url, _) = serve(body.clone(), None).await;
        let (_d, mgr, dl) = setup();
        let mut m = manifest(&url, &body);
        m.files[0].sha256 = "0".repeat(64);
        let err = dl.download(&m, |_| {}).await.unwrap_err();
        assert!(matches!(err, DownloadError::Integrity(_)));
        assert!(!mgr.model_dir(&m.id).join("file.bin").exists());
        assert!(!mgr.model_dir(&m.id).join("file.bin.part").exists());
    }

    #[test]
    fn free_space_requirement_includes_reserve() {
        assert!(required_free_space(10 * GIB) > 10 * GIB + WORKING_SPACE_RESERVE);
        assert_eq!(required_free_space(0), WORKING_SPACE_RESERVE + GIB / 2);
    }
}
