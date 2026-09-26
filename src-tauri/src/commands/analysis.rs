use tauri::State;

use crate::error::{AppError, AppResult};
use crate::llm::model_manager::{self, LlmChoice};
use crate::meetings::analysis::{self, ActionItemPatch, AnalysisView};
use crate::state::AppState;

/// Queue (re)generation of meeting notes, e.g. after transcript edits.
#[tauri::command]
pub fn analyse_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<()> {
    let has_transcript: i64 = state.db.with(|c| {
        c.query_row("SELECT count(*) FROM transcript_segments WHERE meeting_id = ?1", [&meeting_id], |r| r.get(0))
    })?;
    if has_transcript == 0 {
        return Err(AppError::user("no_transcript", "There is no transcript to summarise yet."));
    }
    state.analysis.enqueue(&meeting_id);
    Ok(())
}

#[tauri::command]
pub fn get_analysis(state: State<'_, AppState>, meeting_id: String) -> AppResult<Option<AnalysisView>> {
    let settings = state.settings.read().clone();
    Ok(analysis::load_view(&state.db, &settings, &meeting_id)?)
}

#[tauri::command]
pub fn update_action_item(state: State<'_, AppState>, action_item_id: String, patch: ActionItemPatch) -> AppResult<()> {
    analysis::update_action(&state.db, &action_item_id, &patch).map_err(|e| AppError::user("invalid", e.to_string()))
}

#[tauri::command]
pub fn add_action_item(state: State<'_, AppState>, meeting_id: String, title: String) -> AppResult<String> {
    analysis::add_manual_action(&state.db, &meeting_id, &title).map_err(|e| AppError::user("invalid", e.to_string()))
}

#[tauri::command]
pub fn list_llm_choices(state: State<'_, AppState>) -> Vec<LlmChoice> {
    model_manager::choices(&state.db, &state.models)
}

/// Import a user-provided GGUF (advanced). Copies the file into the models folder.
#[tauri::command]
pub async fn import_gguf(state: State<'_, AppState>, path: String) -> AppResult<LlmChoice> {
    let db = state.db.clone();
    let dir = state.models.models_dir();
    tauri::async_runtime::spawn_blocking(move || model_manager::import_gguf(&db, &dir, std::path::Path::new(&path)))
        .await
        .map_err(|e| AppError::Internal(e.into()))?
        .map_err(|e| AppError::user("import_failed", e.to_string()))
}
