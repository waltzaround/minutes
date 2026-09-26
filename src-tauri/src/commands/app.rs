use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::storage::settings::AppSettings;

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub log_dir: String,
    pub models_dir: String,
    pub debug_build: bool,
    pub platform: String,
}

#[tauri::command]
pub fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        data_dir: state.paths.data_dir.display().to_string(),
        log_dir: state.paths.log_dir.display().to_string(),
        models_dir: state.models_dir().display().to_string(),
        debug_build: cfg!(debug_assertions),
        platform: std::env::consts::OS.into(),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.settings.read().clone()
}

#[tauri::command]
pub fn update_settings(state: State<'_, AppState>, settings: AppSettings) -> AppResult<AppSettings> {
    let mut settings = settings;
    if !cfg!(debug_assertions) {
        settings.advanced.simulate_hardware_fixture = None;
    }
    if let Some(dir) = &settings.advanced.models_dir {
        let p = std::path::Path::new(dir);
        if !p.is_absolute() {
            return Err(AppError::user("invalid_path", "The models folder must be a full path."));
        }
    }
    settings.save(&state.db)?;
    *state.settings.write() = settings.clone();
    state.models.set_models_dir(state.models_dir());
    Ok(settings)
}

#[tauri::command]
pub fn complete_onboarding(state: State<'_, AppState>) -> AppResult<AppSettings> {
    let mut s = state.settings.read().clone();
    s.onboarding.completed = true;
    s.onboarding.completed_at = Some(crate::storage::now());
    s.save(&state.db)?;
    *state.settings.write() = s.clone();
    Ok(s)
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StorageUsage {
    pub models_dir: String,
    pub models_bytes: u64,
    pub audio_bytes: u64,
    pub database_bytes: u64,
    pub free_bytes: u64,
}

fn dir_size(p: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.filter_map(Result::ok)
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

#[tauri::command]
pub async fn storage_usage(state: State<'_, AppState>) -> AppResult<StorageUsage> {
    let models_dir = state.models_dir();
    let meetings = state.paths.meetings_dir.clone();
    let data = state.paths.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || StorageUsage {
        free_bytes: crate::system::capabilities::detect_disk(&models_dir).available_bytes,
        models_bytes: dir_size(&models_dir),
        audio_bytes: dir_size(&meetings),
        database_bytes: ["minutes.db", "minutes.db-wal"].iter().map(|f| std::fs::metadata(data.join(f)).map(|m| m.len()).unwrap_or(0)).sum(),
        models_dir: models_dir.display().to_string(),
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))
}

/// Write a diagnostics report for support. Contains hardware, capability,
/// benchmark and model information plus recent log lines. It never includes
/// transcripts, audio, voiceprints or credentials.
#[tauri::command]
pub async fn export_diagnostics(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let system = state.system.clone();
    let settings = state.settings.read().clone();
    let models = state.models.list();
    let log_dir = state.paths.log_dir.clone();
    let runtime_version = std::fs::read_to_string(state.llama_runtime_dir.join("VERSION")).ok();
    let integrations = serde_json::json!({
        "notionConnected": settings.notion.connected,
        "linearConnected": settings.linear.connected,
    });
    tauri::async_runtime::spawn_blocking(move || -> AppResult<()> {
        let caps = system.capabilities(false, None);
        let mut logs: Vec<std::path::PathBuf> = std::fs::read_dir(&log_dir)
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect())
            .unwrap_or_default();
        logs.sort();
        let tail: Vec<String> = logs
            .last()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.lines().rev().take(400).map(String::from).collect::<Vec<_>>().into_iter().rev().collect())
            .unwrap_or_default();
        let report = serde_json::json!({
            "app": { "version": env!("CARGO_PKG_VERSION"), "debug": cfg!(debug_assertions), "llamaRuntime": runtime_version },
            "capabilities": caps,
            "models": models.iter().map(|m| serde_json::json!({ "id": m.manifest.id, "state": m.state, "bytes": m.bytes_downloaded, "error": m.error })).collect::<Vec<_>>(),
            "settings": { "advanced": settings.advanced, "audio": settings.audio, "privacy": settings.privacy },
            "integrations": integrations,
            "log": tail,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap_or_default())
            .map_err(|e| AppError::user("write_failed", format!("Could not save the report: {e}")))
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?
}
