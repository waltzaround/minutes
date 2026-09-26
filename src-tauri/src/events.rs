//! Backend → frontend events. Names are constants so both sides agree.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

pub const DOWNLOAD_PROGRESS: &str = "download_progress";
pub const AUDIO_LEVEL: &str = "audio_level";
pub const TRANSCRIPT_SEGMENT: &str = "transcript_segment";
pub const PROCESSING_PROGRESS: &str = "processing_progress";
pub const ANALYSIS_PROGRESS: &str = "analysis_progress";
pub const MEETING_WARNING: &str = "meeting_warning";
pub const MEMORY_WARNING: &str = "memory_warning";
pub const MEETING_STATE: &str = "meeting_state";

/// Best-effort emit; a closed window must never break backend work.
pub fn emit<P: Serialize + Clone>(app: &AppHandle, event: &str, payload: P) {
    if let Err(e) = app.emit(event, payload) {
        tracing::debug!(event, error = %e, "emit failed");
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryWarning {
    pub message: String,
    pub available_bytes: u64,
}
