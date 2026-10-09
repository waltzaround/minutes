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
    start_session(&state, title, None)
}

#[tauri::command]
pub fn resume_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<RecordingStatus> {
    start_session(&state, None, Some(meeting_id))
}

fn start_session(state: &AppState, title: Option<String>, resume_id: Option<String>) -> AppResult<RecordingStatus> {
    if state.recorder.is_recording() { return Err(AppError::user("busy", "A session is already recording.")); }
    let settings = state.settings.read().clone();
    let system_available = state
        .system
        .cached_hardware()
        .map(|hw| hw.audio.system_audio_available)
        .unwrap_or(true);
    let opts = StartOptions {
        title,
        resume_id: resume_id.clone(),
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
pub fn pause_meeting(state: State<'_, AppState>) -> AppResult<String> {
    let id = state.recorder.pause()?;
    state.drain_live_transcription();
    Ok(id)
}

#[tauri::command]
pub fn finish_paused_meeting(state: State<'_, AppState>, meeting_id: String) -> AppResult<()> {
    let m = store::get_summary(&state.db, &meeting_id)?.ok_or_else(|| AppError::NotFound("That session".into()))?;
    if m.status != MeetingStatus::Paused { return Err(AppError::user("invalid", "Only paused sessions can be finished.")); }
    store::finish_recording(&state.db, &meeting_id, m.duration_ms.unwrap_or(0), MeetingStatus::Processing)?;
    state.processor.enqueue(crate::meetings::processor::Job { meeting_id, live: None });
    Ok(())
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
    let m = store::get_summary(&state.db, &meeting_id)?.ok_or_else(|| AppError::NotFound("That meeting".into()))?;
    if matches!(m.status, MeetingStatus::Recording | MeetingStatus::Paused | MeetingStatus::Processing) {
        return Err(AppError::user("busy", "Finish the session before transcribing again."));
    }
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

/// Decode a user-selected audio/video file locally; only the extracted audio
/// enters the normal pipeline and its retention policy. Never modify the input.
#[tauri::command]
pub async fn import_media(state: State<'_, AppState>, path: String) -> AppResult<String> {
    import_media_file(&state, path).await
}

async fn import_media_file(state: &AppState, path: String) -> AppResult<String> {
    if state.recorder.is_recording() { return Err(AppError::user("busy", "Pause or finish the recording before importing a file.")); }
    let input = std::path::PathBuf::from(path);
    if !input.is_file() { return Err(AppError::user("invalid_file", "Choose an audio or video file.")); }
    let retention = state.settings.read().privacy.audio_retention.as_db().to_string();
    let title = input.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported recording");
    let (id, _, dir) = store::create_meeting(&state.db, title, &state.paths.meetings_dir, false, true, &retention)?;
    // Keep imports out of crash-recovery's recording state while decoding.
    store::set_status(&state.db, &id, MeetingStatus::Interrupted)?;
    let result = async {
        tokio::fs::create_dir_all(&dir).await.map_err(|e| AppError::Internal(e.into()))?;
        let output = dir.join("system-0.wav");
        crate::audio::media::extract_audio(&state.ffmpeg_runtime_dir, &input, &output).await?;
        let duration = wav::duration_ms(&output).filter(|d| *d > 0)
            .ok_or_else(|| AppError::user("empty_audio", "This file has no audio to transcribe."))?;
        let info = crate::audio::capture::TrackInfo {
            source: AudioSource::System, device_id: None, device_name: "Imported media".into(),
            sample_rate: 16000, channels: 1, path: output, start_offset_ms: 0,
        };
        let track = store::insert_track(&state.db, &id, &info, 0)?;
        store::update_track(&state.db, &track, Some(duration * 16), Some("complete"), None)?;
        store::finish_recording(&state.db, &id, duration, MeetingStatus::Processing)?;
        Ok::<(), AppError>(())
    }.await;
    if let Err(e) = result {
        let _ = store::delete_meeting(&state.db, &id, &state.paths.meetings_dir);
        return Err(e);
    }
    state.processor.enqueue(crate::meetings::processor::Job { meeting_id: id.clone(), live: None });
    Ok(id)
}


#[cfg(test)]
mod workflow_tests {
    use super::*;
    use crate::state::AppPaths;
    use crate::storage::settings::{AppSettings, AudioRetention};
    use std::sync::Arc;

    fn isolated_state(root: &std::path::Path, real_models: bool) -> AppState {
        let db = crate::storage::Database::open(&root.join("minutes.db")).unwrap();
        let mut settings = AppSettings::default();
        settings.privacy.audio_retention = AudioRetention::KeepForever;
        if real_models {
            settings.advanced.models_dir = Some(std::env::var("MINUTES_TEST_MODELS").expect("set MINUTES_TEST_MODELS to installed model folder"));
        }
        settings.save(&db).unwrap();
        let state = AppState::new(AppPaths {
            data_dir: root.into(), log_dir: root.join("logs"), meetings_dir: root.join("meetings"),
            default_models_dir: root.join("models"),
        }, Arc::new(|_, _| {}), root.join("uninstalled-llama")).unwrap();
        if real_models {
            for id in [crate::models::catalog::ASR_ID, crate::models::catalog::VAD_ID] {
                let m = crate::models::catalog::manifest_by_id(id).unwrap();
                assert!(state.models.files_present(m), "missing model files for {id}");
                state.models.record(m, crate::models::manager::InstallState::Installed, m.byte_size, None).unwrap();
            }
        }
        state
    }

    async fn wait_ready(state: &AppState, id: &str) -> MeetingDetail {
        for _ in 0..600 {
            let m = store::get_detail(&state.db, id).unwrap().unwrap();
            if m.summary.status != MeetingStatus::Processing {
                assert_eq!(m.summary.status, MeetingStatus::Ready, "{:?}", m.processing_error);
                return m;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("processing timed out");
    }

    #[tokio::test]
    #[ignore = "requires bundled FFmpeg and installed ASR/VAD models; set MINUTES_TEST_MODELS"]
    async fn video_import_real_transcription_and_invalid_files() {
        let root = tempfile::tempdir().unwrap();
        let state = isolated_state(root.path(), true);
        let sample = std::path::PathBuf::from(std::env::var("MINUTES_TEST_MODELS").unwrap())
            .join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav");
        let video = root.path().join("Class recording.mp4");
        let result = tokio::process::Command::new(crate::audio::media::runtime_binary(&state.ffmpeg_runtime_dir)).args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=160x120:r=10"])
            .arg("-i").arg(&sample).args(["-c:v", "mpeg4", "-c:a", "aac", "-shortest"]).arg(&video).output().await.unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let original = std::fs::read(&video).unwrap();
        let id = import_media_file(&state, video.display().to_string()).await.unwrap();
        let m = wait_ready(&state, &id).await;
        assert_eq!(m.summary.title, "Class recording");
        assert!(m.audio_available);
        assert_eq!(m.tracks.len(), 1);
        assert_eq!(m.tracks[0].source, AudioSource::System);
        assert_eq!(m.tracks[0].sample_rate, 16000);
        assert!(m.segments.iter().all(|s| !s.is_provisional));
        let text = m.segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
        println!("Imported video transcript: {text}");
        assert!(text.to_lowercase().contains("country"), "{text}");
        assert_eq!(std::fs::read(&video).unwrap(), original);
        let bad = root.path().join("broken.mp4");
        std::fs::write(&bad, b"not a video").unwrap();
        assert!(import_media_file(&state, bad.display().to_string()).await.is_err());
        let silent = root.path().join("silent.mp4");
        let result = tokio::process::Command::new(crate::audio::media::runtime_binary(&state.ffmpeg_runtime_dir)).args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=160x120:d=1", "-an", "-c:v", "mpeg4"]).arg(&silent).output().await.unwrap();
        assert!(result.status.success());
        assert!(import_media_file(&state, silent.display().to_string()).await.is_err());
        assert!(import_media_file(&state, root.path().join("missing.mp4").display().to_string()).await.is_err());
        assert_eq!(store::list_recent(&state.db, 50).unwrap().len(), 1, "failed imports must not leave sessions behind");
    }

    #[tokio::test]
    #[ignore = "requires installed ASR/VAD models; set MINUTES_TEST_MODELS"]
    async fn paused_chunks_rebuild_without_duplicate_live_segments() {
        let root = tempfile::tempdir().unwrap();
        let state = isolated_state(root.path(), true);
        let sample = std::path::PathBuf::from(std::env::var("MINUTES_TEST_MODELS").unwrap())
            .join("parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav");
        let duration = wav::duration_ms(&sample).unwrap();
        let (id, _, dir) = store::create_meeting(&state.db, "Paused class", &state.paths.meetings_dir, false, true, "keep_forever").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..2 {
            let path = dir.join(format!("system-{index}.wav"));
            std::fs::copy(&sample, &path).unwrap();
            let spec = hound::WavReader::open(&path).unwrap().spec();
            let info = crate::audio::capture::TrackInfo {
                source: AudioSource::System, device_id: None, device_name: "Test class".into(),
                sample_rate: spec.sample_rate, channels: spec.channels, path, start_offset_ms: duration * index as u64,
            };
            let tid = store::insert_track(&state.db, &id, &info, index).unwrap();
            store::update_track(&state.db, &tid, Some(duration * spec.sample_rate as u64 / 1000), Some("complete"), None).unwrap();
        }
        crate::meetings::transcript::insert_segment(&state.db, &crate::meetings::transcript::NewSegment {
            meeting_id: &id, source: crate::meetings::transcript::SegmentSource::System,
            start_ms: 0, end_ms: 1000, text: "PROVISIONAL PLACEHOLDER", words: &[], person_id: None, speaker_confidence: None,
        }).unwrap();
        store::finish_recording(&state.db, &id, duration, MeetingStatus::Paused).unwrap();
        assert!(store::mark_interrupted(&state.db).unwrap().is_empty());
        store::finish_recording(&state.db, &id, duration * 2, MeetingStatus::Processing).unwrap();
        state.processor.enqueue(crate::meetings::processor::Job { meeting_id: id.clone(), live: None });
        let m = wait_ready(&state, &id).await;
        assert_eq!(m.segments.len(), 2, "one utterance per chunk; no duplicate preview");
        assert!(m.segments.iter().all(|s| s.text.to_lowercase().contains("country") && !s.is_provisional));
        assert!(m.segments[1].start_ms >= duration);
        assert!(m.segments[0].end_ms <= m.segments[1].start_ms);
        println!("Paused class: {} chunks, {} final segments, {} ms", m.tracks.len(), m.segments.len(), m.summary.duration_ms.unwrap());
    }
}
