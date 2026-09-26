use tauri::{AppHandle, State};

use crate::error::{AppError, AppResult};
use crate::events::{self, DOWNLOAD_PROGRESS};
use crate::models::catalog;
use crate::models::download::{self, DownloadError};
use crate::models::manager::{InstallState, ModelStatus};
use crate::state::AppState;

#[tauri::command]
pub fn list_models(state: State<'_, AppState>) -> Vec<ModelStatus> {
    state.models.list()
}

/// Start (or resume) a download in the background. Progress arrives as
/// `download_progress` events. Only ever called from an explicit user action.
#[tauri::command]
pub fn download_model(app: AppHandle, state: State<'_, AppState>, model_id: String) -> AppResult<()> {
    let manifest = catalog::manifest_by_id(&model_id).ok_or_else(|| AppError::NotFound("That model".into()))?;
    if state.models.status(manifest).state == InstallState::Installed {
        return Ok(());
    }
    if state.downloader.is_active(&model_id) {
        return Ok(());
    }
    // Fail fast on disk space so the user sees it immediately.
    state.downloader.check_disk(manifest).map_err(|e| match e {
        DownloadError::InsufficientDisk { required, available } => AppError::user(
            "insufficient_disk",
            format!(
                "Not enough free disk space. {} GB is needed (including space for meeting recordings) but only {} GB is free.",
                required.div_ceil(1 << 30),
                available / (1 << 30)
            ),
        ),
        other => AppError::user("download_failed", other.to_string()),
    })?;

    let downloader = state.downloader.clone();
    tauri::async_runtime::spawn(async move {
        let emit_app = app.clone();
        let result = downloader
            .download(manifest, move |p| events::emit(&emit_app, DOWNLOAD_PROGRESS, p))
            .await;
        match result {
            Ok(()) => tracing::info!(model = manifest.id, "model installed"),
            Err(DownloadError::Paused | DownloadError::Cancelled) => {}
            Err(e) => tracing::warn!(model = manifest.id, error = %e, "model download failed"),
        }
    });
    Ok(())
}

#[tauri::command]
pub fn pause_download(state: State<'_, AppState>, model_id: String) {
    state.downloader.pause(&model_id);
}

#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, model_id: String) -> AppResult<()> {
    let m = catalog::manifest_by_id(&model_id).ok_or_else(|| AppError::NotFound("That model".into()))?;
    state.downloader.cancel(m)?;
    Ok(())
}

#[tauri::command]
pub fn remove_model(state: State<'_, AppState>, model_id: String) -> AppResult<()> {
    if state.downloader.is_active(&model_id) {
        return Err(AppError::user("busy", "Stop the download before removing this model."));
    }
    state.models.remove(&model_id)?;
    Ok(())
}

/// Space needed to finish downloading the given models (for the UI).
#[tauri::command]
pub fn required_download_space(state: State<'_, AppState>, model_ids: Vec<String>) -> u64 {
    let remaining: u64 = model_ids
        .iter()
        .filter_map(|id| catalog::manifest_by_id(id))
        .map(|m| {
            let s = state.models.status(m);
            if s.state == InstallState::Installed { 0 } else { m.byte_size.saturating_sub(s.bytes_downloaded) }
        })
        .sum();
    download::required_free_space(remaining)
}
