use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::people::enrollment::{self, EnrollmentResult};
use crate::people::{self, Person, PersonInput};
use crate::speech::embeddings::SherpaEmbedder;
use crate::state::AppState;
use crate::storage::secrets::VoiceprintCipher;

fn user(e: anyhow::Error) -> AppError {
    AppError::user("invalid", e.to_string())
}

#[tauri::command]
pub fn list_people(state: State<'_, AppState>) -> AppResult<Vec<Person>> {
    Ok(people::list(&state.db)?)
}

#[tauri::command]
pub fn create_person(state: State<'_, AppState>, input: PersonInput) -> AppResult<Person> {
    people::create(&state.db, &input).map_err(user)
}

#[tauri::command]
pub fn update_person(state: State<'_, AppState>, person_id: String, input: PersonInput) -> AppResult<Person> {
    people::update(&state.db, &person_id, &input).map_err(user)
}

#[tauri::command]
pub fn delete_person(state: State<'_, AppState>, person_id: String) -> AppResult<()> {
    Ok(people::delete(&state.db, &person_id)?)
}

/// Write the people directory (no voice data) to a user-chosen file.
#[tauri::command]
pub fn export_people(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let data = people::export(&state.db)?;
    let json = serde_json::to_string_pretty(&data).map_err(|e| AppError::Internal(e.into()))?;
    std::fs::write(&path, json).map_err(|e| AppError::user("write_failed", format!("Could not save the file: {e}")))?;
    Ok(())
}

#[tauri::command]
pub fn start_voice_enrollment(state: State<'_, AppState>, person_id: String) -> AppResult<()> {
    if state.recorder.is_recording() {
        return Err(AppError::user("busy", "Finish the current meeting before setting up voice recognition."));
    }
    people::get(&state.db, &person_id)?.ok_or_else(|| AppError::NotFound("That person".into()))?;
    if state.speech.embedding_model().is_none() || state.speech.vad_model().is_none() {
        return Err(AppError::user("models_missing", "Voice recognition needs the speech models. Download them in Settings → Models."));
    }
    let mic = state.settings.read().audio.microphone_device_id.clone();
    state
        .enrollment
        .start(&person_id, mic, &state.paths.data_dir.join("enrollment"), state.sink.clone())
        .map_err(|e| AppError::user("mic_failed", format!("The microphone could not be started: {e}")))
}

#[tauri::command]
pub fn cancel_voice_enrollment(state: State<'_, AppState>) {
    state.enrollment.cancel();
}

#[tauri::command]
pub async fn finish_voice_enrollment(state: State<'_, AppState>, keep_recording: bool) -> AppResult<EnrollmentResult> {
    let (person_id, path) = state.enrollment.stop().map_err(user)?;
    let vad = state.speech.vad_model().ok_or_else(|| AppError::user("models_missing", "The speech detection model is missing."))?;
    let emb_model = state.speech.embedding_model().ok_or_else(|| AppError::user("models_missing", "The voice recognition model is missing."))?;
    let db = state.db.clone();
    let secrets = state.secrets.clone();
    let kept_dir = state.paths.data_dir.join("enrollment").join("kept");
    tauri::async_runtime::spawn_blocking(move || -> AppResult<EnrollmentResult> {
        let embedder = SherpaEmbedder::load(&emb_model, 2).map_err(user)?;
        let analysis = enrollment::analyse_recording(&path, &vad, &embedder);
        let kept_at = if keep_recording {
            let _ = std::fs::create_dir_all(&kept_dir);
            let dest = kept_dir.join(format!("{person_id}-{}.wav", chrono::Utc::now().format("%Y%m%d%H%M%S")));
            std::fs::rename(&path, &dest).ok().map(|_| dest.display().to_string())
        } else {
            let _ = std::fs::remove_file(&path);
            None
        };
        let (result, speech_seconds) = analysis.map_err(user)?;
        match result {
            Ok((embeddings, consistency)) => {
                let cipher = VoiceprintCipher::load_or_create(secrets.as_ref())?;
                people::add_embeddings(&db, &cipher, &person_id, &embeddings, "enrollment", None, Some(consistency))?;
                Ok(EnrollmentResult {
                    accepted: true,
                    speech_seconds,
                    samples: embeddings.len() as u32,
                    consistency: Some(consistency),
                    message: None,
                    recording_kept_at: kept_at,
                })
            }
            Err(message) => Ok(EnrollmentResult {
                accepted: false,
                speech_seconds,
                samples: 0,
                consistency: None,
                message: Some(message),
                recording_kept_at: kept_at,
            }),
        }
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?
}

#[tauri::command]
pub fn delete_voice_profile(state: State<'_, AppState>, person_id: String) -> AppResult<()> {
    Ok(people::delete_voice_profile(&state.db, &person_id)?)
}

#[tauri::command]
pub fn clear_all_voice_data(state: State<'_, AppState>) -> AppResult<()> {
    people::clear_all_voice_data(&state.db, state.secrets.as_ref()).map_err(|e| AppError::Internal(e))
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IdentityUpdate {
    /// Voiceprints added from confirmed speech in this meeting.
    pub profile_samples_added: u32,
}

/// Map a meeting speaker to a person (or clear it). With `improve_profile`,
/// confirmed speech from this meeting strengthens that person's voice
/// profile — only ever on explicit user request.
#[tauri::command]
pub async fn update_speaker_identity(
    state: State<'_, AppState>,
    cluster_id: String,
    person_id: Option<String>,
    improve_profile: bool,
) -> AppResult<IdentityUpdate> {
    crate::meetings::speakers::set_cluster_person(&state.db, &cluster_id, person_id.as_deref())?;
    let mut added = 0;
    if let (true, Some(pid)) = (improve_profile, person_id) {
        let db = state.db.clone();
        let engine = state.speech.clone();
        let secrets = state.secrets.clone();
        added = tauri::async_runtime::spawn_blocking(move || -> AppResult<u32> {
            let (meeting_id, embs) = crate::meetings::speakers::confirmed_speech_embeddings(&db, &engine, &cluster_id).map_err(user)?;
            if embs.is_empty() {
                return Ok(0);
            }
            let cipher = VoiceprintCipher::load_or_create(secrets.as_ref())?;
            people::add_embeddings(&db, &cipher, &pid, &embs, "confirmed_meeting", Some(&meeting_id), None)?;
            Ok(embs.len() as u32)
        })
        .await
        .map_err(|e| AppError::Internal(e.into()))??;
    }
    Ok(IdentityUpdate { profile_samples_added: added })
}
