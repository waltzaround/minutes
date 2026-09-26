//! Developer/provisioning tool: install catalog models into an app data
//! directory without the UI (same verified, resumable downloader).
//!
//! cargo run --example install_models -- "<data dir>" parakeet-tdt-0.6b-v3-int8 silero-vad
//!
//! On macOS the app data dir is ~/Library/Application Support/app.minutes.desktop

use std::path::PathBuf;
use std::sync::Arc;

use minutes_lib::models::{catalog, download::Downloader, manager::ModelManager};
use minutes_lib::storage::Database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let data_dir = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("usage: install_models <data dir> <model ids...>"))?);
    let ids: Vec<String> = args.collect();
    let db = Database::open(&data_dir.join("minutes.db"))?;
    let manager = Arc::new(ModelManager::new(db, data_dir.join("models")));
    let downloader = Downloader::new(manager.clone());
    for id in &ids {
        let m = catalog::manifest_by_id(id).ok_or_else(|| anyhow::anyhow!("unknown model {id}"))?;
        println!("installing {} ({} bytes)", m.name, m.byte_size);
        let last = std::sync::atomic::AtomicU64::new(0);
        downloader
            .download(m, |p| {
                let pct = p.bytes_downloaded * 100 / p.total_bytes.max(1);
                if pct != last.swap(pct, std::sync::atomic::Ordering::Relaxed) {
                    eprint!("\r  {pct}%");
                }
            })
            .await?;
        eprintln!("\r  done");
    }
    Ok(())
}
