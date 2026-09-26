use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::audio::capture::AudioSource;
use crate::audio::wav;
use crate::error::{AppError, AppResult};
use crate::meetings::recorder::{RecorderError, RecordingStatus, StartOptions};
use crate::meetings::store::{self, AudioTrackRow, MeetingDetail};
use crate::meetings::{MeetingStatus, MeetingSummary};
use crate::state::AppState;

impl From<RecorderError> for AppError {
    fn from(e: RecorderError) -> Self {
        match e {
            RecorderError::Db(e) => AppError::Database(e),
            other => AppError::user("recording", other.to_string()),
        }
    }
}

#[tauri::command]
pub fn start_meeting(state: State<'_, AppState>, title: Option<String>) -> AppResult<RecordingStatus> {
    let settings = state.settings.read().clone();
    let system_available = state
        .system
        .cached_hardware()
        .map(|hw| hw.audio.system_audio_available)
        .unwrap_or(true);
    let opts = StartOptions {
        title,
        microphone_device_id: settings.audio.microphone_device_id.clone(),
        output_device_id: settings.audio.output_device_id.clone(),
        capture_system: settings.audio.capture_system_audio,
        system_audio_available: system_available,
        retention: settings.privacy.audio_retention.as_db().into(),
        meetings_root: state.paths.meetings_dir.clone(),
        speech: None,
    };
    let realtime = state
        .system
        .cached_hardware()
        .map(|hw| crate::system::profile::assess(&hw, Some(&state.system.load_benchmarks(&hw))).settings.realtime_transcript)
        .unwrap_or(true);
    let mut opts = opts;
    opts.speech = state.begin_live_transcription(realtime);
    match state.recorder.start(opts) {
        Ok(s) => Ok(s),
        Err(e) => {
            state.abandon_live_transcription();
            Err(e.into())
        }
    }
}

#[tauri::command]
pub fn stop_meeting(state: State<'_, AppState>) -> AppResult<String> {
    let id = state.recorder.stop()?;
    state.on_recording_stopped(&id);
    Ok(id)
}

#[tauri::command]
pub fn get_recording_status(state: State<'_, AppState>) -> Option<RecordingStatus> {
    state.recorder.status()
}

#[tauri::command]
pub fn reconnect_source(state: State<'_, AppState>, source: AudioSource) -> AppResult<RecordingStatus> {
    Ok(state.recorder.reconnect(source)?)
}

/// Process a meeting again (e.g. after installing models or a failure).
/// Needs the audio to still be on disk.
#[tauri::command]
pub fn reprocess_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<()> {
    let tracks = state.db.with(|c| store::tracks(c, &meeting_id))?;
    if !tracks.iter().any(|t| std::path::Path::new(&t.path).exists()) {
        return Err(AppError::user("no_audio", "The audio for this meeting has been deleted, so it can't be transcribed again."));
    }
    // Start from a clean transcript; analysis and speakers are rebuilt too.
    state.db.transaction(|tx| {
        tx.execute("DELETE FROM transcript_segments WHERE meeting_id = ?1", [&meeting_id])?;
        tx.execute("DELETE FROM speaker_clusters WHERE meeting_id = ?1", [&meeting_id])?;
        tx.execute("UPDATE meetings SET status = 'processing', processing_error = NULL WHERE id = ?1", [&meeting_id])?;
        Ok(())
    })?;
    state.on_recording_stopped(&meeting_id);
    Ok(())
}

#[tauri::command]
pub fn list_meetings(state: State<'_, AppState>, limit: Option<u32>) -> AppResult<Vec<MeetingSummary>> {
    Ok(store::list_recent(&state.db, limit.unwrap_or(50))?)
}

#[tauri::command]
pub fn rename_meeting(state: State<'_, AppState>, meeting_id: String, title: String) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::user("invalid_title", "The meeting needs a name."));
    }
    Ok(store::rename(&state.db, &meeting_id, title)?)
}

#[tauri::command]
pub fn delete_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<()> {
    if state.recorder.status().is_some_and(|s| s.meeting_id == meeting_id) {
        return Err(AppError::user("busy", "Stop the recording before deleting this meeting."));
    }
    store::delete_meeting(&state.db, &meeting_id, &state.paths.meetings_dir)?;
    Ok(())
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InterruptedMeeting {
    pub meeting: MeetingSummary,
    /// Audio recovered from disk, in milliseconds.
    pub recovered_ms: u64,
    pub tracks: Vec<AudioTrackRow>,
}

/// Meetings that were cut off by a crash or forced quit, with how much audio
/// can be recovered.
#[tauri::command]
pub fn list_interrupted_meetings(state: State<'_, AppState>) -> AppResult<Vec<InterruptedMeeting>> {
    let all = store::list_recent(&state.db, 500)?;
    let mut out = Vec::new();
    for m in all.into_iter().filter(|m| m.status == MeetingStatus::Interrupted) {
        let tracks = state.db.with(|c| store::tracks(c, &m.id))?;
        let recovered_ms = tracks
            .iter()
            .filter_map(|t| {
                let p = std::path::Path::new(&t.path);
                if !p.exists() {
                    return None;
                }
                let _ = wav::repair_header(p);
                wav::duration_ms(p).map(|d| d + t.start_offset_ms)
            })
            .max()
            .unwrap_or(0);
        out.push(InterruptedMeeting { meeting: m, recovered_ms, tracks });
    }
    Ok(out)
}

/// Repair audio files of an interrupted meeting and queue it for processing.
#[tauri::command]
pub fn recover_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<()> {
    let tracks = state.db.with(|c| store::tracks(c, &meeting_id))?;
    let mut duration = 0;
    for t in &tracks {
        let p = std::path::Path::new(&t.path);
        if p.exists() {
            match wav::repair_header(p) {
                Ok(_) => {
                    let d = wav::duration_ms(p).unwrap_or(0);
                    duration = duration.max(d + t.start_offset_ms);
                    let frames = d * t.sample_rate as u64 / 1000;
                    store::update_track(&state.db, &t.id, Some(frames), Some("complete"), None)?;
                }
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "could not repair audio file");
                    store::update_track(&state.db, &t.id, None, Some("failed"), Some("The audio file was damaged."))?;
                }
            }
        }
    }
    if duration == 0 {
        return Err(AppError::user("nothing_recovered", "No audio could be recovered from this meeting."));
    }
    store::finish_recording(&state.db, &meeting_id, duration, MeetingStatus::Processing)?;
    state.on_recording_stopped(&meeting_id);
    Ok(())
}

#[tauri::command]
pub fn get_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<MeetingDetail> {
    store::get_detail(&state.db, &meeting_id)?.ok_or_else(|| AppError::NotFound("That meeting".into()))
}

#[tauri::command]
pub fn list_speakers(state: State<'_, AppState>, meeting_id: String) -> AppResult<Vec<crate::meetings::speakers::SpeakerCluster>> {
    Ok(crate::meetings::speakers::list_clusters(&state.db, &meeting_id)?)
}

#[tauri::command]
pub fn rename_speaker(state: State<'_, AppState>, cluster_id: String, label: String) -> AppResult<()> {
    if label.trim().is_empty() {
        return Err(AppError::user("invalid", "The speaker needs a name."));
    }
    Ok(crate::meetings::speakers::rename_cluster(&state.db, &cluster_id, &label)?)
}

#[tauri::command]
pub fn merge_speakers(state: State<'_, AppState>, from_cluster_id: String, into_cluster_id: String) -> AppResult<()> {
    crate::meetings::speakers::merge_clusters(&state.db, &from_cluster_id, &into_cluster_id).map_err(|e| AppError::user("invalid", e.to_string()))
}

#[tauri::command]
pub fn split_segment(state: State<'_, AppState>, segment_id: String, at_word: usize, cluster_id: Option<String>) -> AppResult<()> {
    crate::meetings::speakers::split_segment(&state.db, &segment_id, at_word, cluster_id.as_deref())
        .map_err(|e| AppError::user("invalid", e.to_string()))
}

#[tauri::command]
pub fn edit_segment(state: State<'_, AppState>, segment_id: String, text: String) -> AppResult<()> {
    crate::meetings::speakers::edit_segment_text(&state.db, &segment_id, &text).map_err(|e| AppError::user("invalid", e.to_string()))
}

#[tauri::command]
pub fn search_meetings(state: State<'_, AppState>, query: String) -> AppResult<Vec<store::SearchHit>> {
    Ok(store::search(&state.db, &query, 50)?)
}
