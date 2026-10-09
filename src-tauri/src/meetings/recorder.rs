//! Recording sessions: start/stop, both capture streams, warnings, periodic
//! persistence and reconnects.
//!
//! The recorder owns the capture handles. A small "pump" thread translates
//! capture events into UI events and database updates, so neither the audio
//! callback nor the capture workers ever touch Tauri or SQLite.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use super::store;
use super::MeetingStatus;
use crate::audio::capture::{AudioSource, CaptureEvent, CaptureHandle, CaptureRequest, SpeechBlock};
use crate::storage::Database;

/// Emits a named event with a JSON payload (Tauri in the app, a recorder in
/// tests).
pub type EventSink = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SourceState {
    Recording,
    /// Capture stopped because the device disappeared or failed.
    Disconnected,
    /// Could not be started.
    Failed,
    /// Not requested for this meeting.
    Off,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceStatus {
    pub state: SourceState,
    pub device_name: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecordingStatus {
    pub meeting_id: String,
    pub title: String,
    pub started_at: String,
    pub elapsed_ms: u64,
    pub microphone: SourceStatus,
    pub system: SourceStatus,
    pub warnings: Vec<MeetingWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WarningKind {
    SourceFailed,
    SourceDisconnected,
    /// System audio has been digital silence while the microphone hears
    /// speech; likely a missing macOS permission or wrong output device.
    SystemAudioSilent,
    AudioDropped,
    LiveTranscriptBehind,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MeetingWarning {
    pub meeting_id: String,
    pub kind: WarningKind,
    pub source: Option<AudioSource>,
    pub message: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioLevel {
    pub source: AudioSource,
    /// 0..1, perceptually scaled for meters.
    pub level: f32,
    pub active: bool,
}

pub struct StartOptions {
    pub title: Option<String>,
    pub resume_id: Option<String>,
    pub microphone_device_id: Option<String>,
    pub output_device_id: Option<String>,
    pub capture_system: bool,
    pub system_audio_available: bool,
    pub retention: String,
    pub meetings_root: PathBuf,
    pub speech: Option<SyncSender<SpeechBlock>>,
}

struct Track {
    handle: Option<CaptureHandle>,
    track_id: String,
    segment: u32,
}

struct Shared {
    meeting_id: String,
    started: Instant,
    offset_ms: u64,
    status: Mutex<HashMap<AudioSource, SourceStatus>>,
    warnings: Mutex<Vec<MeetingWarning>>,
    track_ids: Mutex<HashMap<AudioSource, String>>,
}

struct Active {
    shared: Arc<Shared>,
    title: String,
    started_at: String,
    audio_dir: PathBuf,
    tracks: HashMap<AudioSource, Track>,
    events_tx: Sender<CaptureEvent>,
    speech: Option<SyncSender<SpeechBlock>>,
    pump_stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    device_ids: HashMap<AudioSource, Option<String>>,
}

pub struct Recorder {
    db: Database,
    sink: EventSink,
    active: Mutex<Option<Active>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("A meeting is already being recorded.")]
    AlreadyRecording,
    #[error("No meeting is being recorded.")]
    NotRecording,
    #[error("Recording could not start: {0}")]
    StartFailed(String),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

/// Map RMS to a 0..1 meter value (−60 dBFS → 0, 0 dBFS → 1).
pub fn meter_level(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

const PUMP_TICK: Duration = Duration::from_millis(200);
const HEARTBEAT: Duration = Duration::from_secs(5);
const SILENCE_CHECK_AFTER: Duration = Duration::from_secs(30);
/// RMS above which the microphone is considered to hear speech.
const SPEECH_RMS: f32 = 0.01;

impl Recorder {
    pub fn new(db: Database, sink: EventSink) -> Self {
        Recorder { db, sink, active: Mutex::new(None) }
    }

    pub fn is_recording(&self) -> bool {
        self.active.lock().is_some()
    }

    pub fn start(&self, opts: StartOptions) -> Result<RecordingStatus, RecorderError> {
        let mut guard = self.active.lock();
        if guard.is_some() {
            return Err(RecorderError::AlreadyRecording);
        }
        let title = opts.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(store::default_title);
        let want_system = opts.capture_system;
        let (meeting_id, started_at, audio_dir, title, offset_ms, next_segment) = if let Some(id) = &opts.resume_id {
            let meeting = store::get_summary(&self.db, id)?.ok_or_else(|| RecorderError::StartFailed("That session could not be found.".into()))?;
            if meeting.status != MeetingStatus::Paused {
                return Err(RecorderError::StartFailed("Only paused sessions can be resumed.".into()));
            }
            let tracks = self.db.with(|c| store::tracks(c, id))?;
            let next = tracks.iter().map(|t| t.segment_index).max().map(|n| n + 1).unwrap_or(0);
            let dir = store::audio_dir(&self.db, id)?.ok_or_else(|| RecorderError::StartFailed("The session's audio folder is missing.".into()))?;
            (id.clone(), meeting.started_at, dir, meeting.title, meeting.duration_ms.unwrap_or(0), next)
        } else {
            let (id, ts, dir) = store::create_meeting(&self.db, &title, &opts.meetings_root, true, want_system, &opts.retention)?;
            (id, ts, dir, title, 0, 0)
        };
        let started = Instant::now();
        let shared = Arc::new(Shared {
            meeting_id: meeting_id.clone(),
            started,
            offset_ms,
            status: Mutex::new(HashMap::new()),
            warnings: Mutex::new(Vec::new()),
            track_ids: Mutex::new(HashMap::new()),
        });
        let (events_tx, events_rx) = mpsc::channel::<CaptureEvent>();

        let mut active = Active {
            shared: shared.clone(),
            title: title.clone(),
            started_at,
            audio_dir,
            tracks: HashMap::new(),
            events_tx,
            speech: opts.speech.clone(),
            pump_stop: Arc::new(AtomicBool::new(false)),
            pump: None,
            device_ids: HashMap::from([
                (AudioSource::Microphone, opts.microphone_device_id.clone()),
                (AudioSource::System, opts.output_device_id.clone()),
            ]),
        };

        self.start_source(&mut active, AudioSource::Microphone, next_segment);
        if want_system {
            if opts.system_audio_available {
                self.start_source(&mut active, AudioSource::System, next_segment);
            } else {
                self.set_source(&shared, AudioSource::System, SourceState::Failed, None, Some("Meeting audio capture is not available on this computer.".into()));
                self.warn(&shared, WarningKind::SourceFailed, Some(AudioSource::System), "Meeting audio is not being recorded because capture is not available on this computer. Only your microphone is recorded.".into());
            }
        } else {
            self.set_source(&shared, AudioSource::System, SourceState::Off, None, None);
        }

        let any_running = active.tracks.values().any(|t| t.handle.is_some());
        if !any_running {
            // Nothing could be captured: do not leave an empty meeting behind.
            let reason = shared
                .status
                .lock()
                .get(&AudioSource::Microphone)
                .and_then(|s| s.message.clone())
                .unwrap_or_else(|| "No audio device could be opened.".into());
            if opts.resume_id.is_none() {
                let _ = store::delete_meeting(&self.db, &meeting_id, &opts.meetings_root);
            }
            return Err(RecorderError::StartFailed(reason));
        }

        self.db.with(|c| c.execute("UPDATE meetings SET status = 'recording', paused = 0, ended_at = NULL WHERE id = ?1", [&meeting_id]))?;
        active.pump = Some(spawn_pump(self.db.clone(), self.sink.clone(), shared.clone(), events_rx, active.pump_stop.clone()));
        let status = snapshot(&active);
        *guard = Some(active);
        (self.sink)(crate::events::MEETING_STATE, serde_json::to_value(&status).unwrap_or_default());
        Ok(status)
    }

    fn start_source(&self, active: &mut Active, source: AudioSource, segment: u32) {
        let shared = active.shared.clone();
        let offset = shared.offset_ms + shared.started.elapsed().as_millis() as u64;
        let path = active.audio_dir.join(format!("{}-{segment}.wav", source.as_str()));
        let req = CaptureRequest {
            source,
            device_id: active.device_ids.get(&source).cloned().flatten(),
            path,
            start_offset_ms: offset,
        };
        match CaptureHandle::start(req, active.events_tx.clone(), active.speech.clone()) {
            Ok(handle) => {
                let track_id = match store::insert_track(&self.db, &shared.meeting_id, &handle.info, segment) {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!(error = %e, "could not record audio track");
                        String::new()
                    }
                };
                shared.track_ids.lock().insert(source, track_id.clone());
                self.set_source(&shared, source, SourceState::Recording, Some(handle.info.device_name.clone()), None);
                active.tracks.insert(source, Track { handle: Some(handle), track_id, segment });
            }
            Err(e) => {
                tracing::warn!(source = source.as_str(), error = %e, "could not start capture");
                let msg = match source {
                    AudioSource::Microphone => format!("Your microphone could not be recorded: {e}"),
                    AudioSource::System => format!("Meeting audio could not be recorded: {e}. Only your microphone is being recorded."),
                };
                self.set_source(&shared, source, SourceState::Failed, None, Some(e.to_string()));
                self.warn(&shared, WarningKind::SourceFailed, Some(source), msg);
            }
        }
    }

    fn set_source(&self, shared: &Shared, source: AudioSource, state: SourceState, device: Option<String>, message: Option<String>) {
        let mut st = shared.status.lock();
        let prev_device = st.get(&source).and_then(|s| s.device_name.clone());
        st.insert(source, SourceStatus { state, device_name: device.or(prev_device), message });
    }

    fn warn(&self, shared: &Shared, kind: WarningKind, source: Option<AudioSource>, message: String) {
        let w = MeetingWarning {
            meeting_id: shared.meeting_id.clone(),
            kind,
            source,
            message,
            at_ms: shared.offset_ms + shared.started.elapsed().as_millis() as u64,
        };
        shared.warnings.lock().push(w.clone());
        (self.sink)(crate::events::MEETING_WARNING, serde_json::to_value(&w).unwrap_or_default());
    }

    pub fn status(&self) -> Option<RecordingStatus> {
        self.active.lock().as_ref().map(snapshot)
    }

    /// Restart capture for a source after a disconnect. Audio continues in a
    /// new segment file aligned to the meeting timeline.
    pub fn reconnect(&self, source: AudioSource) -> Result<RecordingStatus, RecorderError> {
        let mut guard = self.active.lock();
        let active = guard.as_mut().ok_or(RecorderError::NotRecording)?;
        let next_segment = match active.tracks.remove(&source) {
            Some(mut t) => {
                if let Some(h) = t.handle.take() {
                    let r = h.stop();
                    let _ = store::update_track(&self.db, &t.track_id, Some(r.frames_written), Some("disconnected"), r.error.as_deref());
                }
                t.segment + 1
            }
            None => self.db.with(|c| store::tracks(c, &active.shared.meeting_id))?
                .iter().filter(|t| t.source == source).map(|t| t.segment_index).max().map(|n| n + 1).unwrap_or(0),
        };
        self.start_source(active, source, next_segment);
        Ok(snapshot(active))
    }

    /// Stop recording, finalise all audio files and mark the meeting as
    /// ready for processing. Returns the meeting id.
    pub fn stop(&self) -> Result<String, RecorderError> {
        self.finish(MeetingStatus::Processing)
    }

    pub fn pause(&self) -> Result<String, RecorderError> {
        self.finish(MeetingStatus::Paused)
    }

    fn finish(&self, status: MeetingStatus) -> Result<String, RecorderError> {
        let mut active = self.active.lock().take().ok_or(RecorderError::NotRecording)?;
        let duration = active.shared.offset_ms + active.shared.started.elapsed().as_millis() as u64;
        for (source, mut t) in active.tracks.drain() {
            if let Some(h) = t.handle.take() {
                let r = h.stop();
                let status = if r.error.is_some() { "failed" } else { "complete" };
                if let Err(e) = store::update_track(&self.db, &t.track_id, Some(r.frames_written), Some(status), r.error.as_deref()) {
                    tracing::error!(error = %e, "could not update track");
                }
                if r.dropped_frames > 0 {
                    tracing::warn!(source = source.as_str(), dropped = r.dropped_frames, "audio frames were dropped");
                }
            } else {
                let _ = store::update_track(&self.db, &t.track_id, None, Some("disconnected"), None);
            }
        }
        active.pump_stop.store(true, Ordering::Relaxed);
        if let Some(p) = active.pump.take() {
            let _ = p.join();
        }
        let id = active.shared.meeting_id.clone();
        store::finish_recording(&self.db, &id, duration, status)?;
        (self.sink)(crate::events::MEETING_STATE, serde_json::json!({ "meetingId": id, "stopped": true }));
        Ok(id)
    }
}

fn snapshot(a: &Active) -> RecordingStatus {
    let st = a.shared.status.lock();
    let get = |s: AudioSource| {
        st.get(&s).cloned().unwrap_or(SourceStatus { state: SourceState::Off, device_name: None, message: None })
    };
    RecordingStatus {
        meeting_id: a.shared.meeting_id.clone(),
        title: a.title.clone(),
        started_at: a.started_at.clone(),
        elapsed_ms: a.shared.offset_ms + a.shared.started.elapsed().as_millis() as u64,
        microphone: get(AudioSource::Microphone),
        system: get(AudioSource::System),
        warnings: a.shared.warnings.lock().clone(),
    }
}

fn spawn_pump(
    db: Database,
    sink: EventSink,
    shared: Arc<Shared>,
    rx: mpsc::Receiver<CaptureEvent>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("recording-pump".into())
        .spawn(move || {
            let mut last_heartbeat = Instant::now();
            let mut mic_speech = Duration::ZERO;
            let mut system_signal_seen = false;
            let mut silence_warned = false;
            let mut drop_warned = false;
            let warn = |kind: WarningKind, source: Option<AudioSource>, message: String| {
                let w = MeetingWarning {
                    meeting_id: shared.meeting_id.clone(),
                    kind,
                    source,
                    message,
                    at_ms: shared.offset_ms + shared.started.elapsed().as_millis() as u64,
                };
                shared.warnings.lock().push(w.clone());
                sink(crate::events::MEETING_WARNING, serde_json::to_value(&w).unwrap_or_default());
            };
            loop {
                match rx.recv_timeout(PUMP_TICK) {
                    Ok(CaptureEvent::Level { source, rms, peak: _, any_signal }) => {
                        if source == AudioSource::Microphone && rms > SPEECH_RMS {
                            mic_speech += Duration::from_millis(80);
                        }
                        if source == AudioSource::System && any_signal {
                            system_signal_seen = true;
                        }
                        let lvl = AudioLevel { source, level: meter_level(rms), active: rms > SPEECH_RMS / 2.0 };
                        sink(crate::events::AUDIO_LEVEL, serde_json::to_value(&lvl).unwrap_or_default());
                    }
                    Ok(CaptureEvent::Failed { source, message, disconnected }) => {
                        let already = shared
                            .status
                            .lock()
                            .get(&source)
                            .is_some_and(|s| s.state == SourceState::Disconnected);
                        if already {
                            continue;
                        }
                        let at = shared.offset_ms + shared.started.elapsed().as_millis() as u64;
                        {
                            let mut st = shared.status.lock();
                            let prev = st.get(&source).and_then(|s| s.device_name.clone());
                            st.insert(
                                source,
                                SourceStatus {
                                    state: SourceState::Disconnected,
                                    device_name: prev,
                                    message: Some(message.clone()),
                                },
                            );
                        }
                        if let Some(track) = shared.track_ids.lock().get(&source) {
                            let _ = store::update_track(&db, track, None, Some("disconnected"), Some(&message));
                        }
                        let clock = format!("{}:{:02}", at / 60_000, (at / 1000) % 60);
                        let text = match (source, disconnected) {
                            (AudioSource::System, _) => {
                                format!("Meeting audio disconnected at {clock}. Microphone recording is still active.")
                            }
                            (AudioSource::Microphone, _) => {
                                format!("Microphone disconnected at {clock}. Meeting audio is still being recorded if it was on.")
                            }
                        };
                        warn(
                            if disconnected { WarningKind::SourceDisconnected } else { WarningKind::SourceFailed },
                            Some(source),
                            text,
                        );
                    }
                    Ok(CaptureEvent::Overrun { source, dropped_frames }) => {
                        tracing::warn!(source = source.as_str(), dropped_frames, "capture overrun");
                        if !drop_warned {
                            drop_warned = true;
                            warn(
                                WarningKind::AudioDropped,
                                Some(source),
                                "Your computer is under heavy load and a moment of audio was lost. Closing other apps can help.".into(),
                            );
                        }
                    }
                    Ok(CaptureEvent::Progress { source, frames_written }) => {
                        if let Some(track) = shared.track_ids.lock().get(&source) {
                            let _ = store::update_track(&db, track, Some(frames_written), None, None);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if last_heartbeat.elapsed() >= HEARTBEAT {
                    last_heartbeat = Instant::now();
                    let _ = store::touch(&db, &shared.meeting_id);
                }
                let system_recording = shared
                    .status
                    .lock()
                    .get(&AudioSource::System)
                    .is_some_and(|s| s.state == SourceState::Recording);
                if !silence_warned
                    && system_recording
                    && !system_signal_seen
                    && shared.started.elapsed() >= SILENCE_CHECK_AFTER
                    && mic_speech >= Duration::from_secs(5)
                {
                    silence_warned = true;
                    let msg = if cfg!(target_os = "macos") {
                        "No meeting audio is arriving. If other people are talking, allow “System Audio Recording” for Minutes in System Settings → Privacy & Security, or check that the call plays through the selected output."
                    } else {
                        "No meeting audio is arriving. If other people are talking, check that the call plays through the selected output device."
                    };
                    warn(WarningKind::SystemAudioSilent, Some(AudioSource::System), msg.into());
                }
                if stop.load(Ordering::Relaxed) {
                    // Drain whatever the workers sent while stopping.
                    while let Ok(CaptureEvent::Progress { source, frames_written }) = rx.try_recv() {
                        if let Some(track) = shared.track_ids.lock().get(&source) {
                            let _ = store::update_track(&db, track, Some(frames_written), None, None);
                        }
                    }
                    break;
                }
            }
        })
        .expect("spawn recording pump")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_is_log_scaled() {
        assert_eq!(meter_level(0.0), 0.0);
        assert!((meter_level(1.0) - 1.0).abs() < 1e-6);
        assert!((meter_level(0.001) - 0.0).abs() < 1e-6); // −60 dBFS
        assert!((meter_level(0.0316) - 0.5).abs() < 0.01); // −30 dBFS
    }

    /// Hardware regression test using disposable recordings and a DB reopen.
    #[test]
    #[ignore = "records two short samples from the default microphone"]
    fn real_device_pause_resume_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("minutes.db");
        let db = Database::open(&db_path).unwrap();
        let opts = |resume_id| StartOptions {
            title: Some("Hardware pause test".into()), resume_id,
            microphone_device_id: None, output_device_id: None,
            capture_system: false, system_audio_available: false,
            retention: "keep_forever".into(), meetings_root: root.path().join("meetings"), speech: None,
        };
        let rec = Recorder::new(db.clone(), Arc::new(|_, _| {}));
        let first = rec.start(opts(None)).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(rec.pause().unwrap(), first.meeting_id);
        assert!(!rec.is_recording());
        let saved = store::get_summary(&db, &first.meeting_id).unwrap().unwrap();
        assert_eq!(saved.status, MeetingStatus::Paused);
        let tracks = db.with(|c| store::tracks(c, &first.meeting_id)).unwrap();
        let original = std::fs::read(&tracks[0].path).unwrap();
        drop(rec);
        drop(db);
        std::thread::sleep(Duration::from_secs(2));
        let db = Database::open(&db_path).unwrap();
        assert!(store::mark_interrupted(&db).unwrap().is_empty());
        let rec = Recorder::new(db.clone(), Arc::new(|_, _| {}));
        let resumed = rec.start(opts(Some(first.meeting_id.clone()))).unwrap();
        assert_eq!(resumed.meeting_id, first.meeting_id);
        assert!(resumed.elapsed_ms < saved.duration_ms.unwrap() + 1500, "break time was included");
        std::thread::sleep(Duration::from_millis(1200));
        rec.stop().unwrap();
        let tracks = db.with(|c| store::tracks(c, &first.meeting_id)).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].segment_index, 0);
        assert_eq!(tracks[1].segment_index, 1);
        assert!(tracks[1].start_offset_ms >= saved.duration_ms.unwrap());
        assert_eq!(std::fs::read(&tracks[0].path).unwrap(), original, "resume overwrote the first chunk");
        for t in &tracks {
            assert!(crate::audio::wav::duration_ms(std::path::Path::new(&t.path)).unwrap() >= 800);
        }
        assert_eq!(store::get_summary(&db, &first.meeting_id).unwrap().unwrap().status, MeetingStatus::Processing);
        println!("Microphone pause/reopen/resume: two intact chunks, break excluded");
    }

    /// Records from the real default devices for one second. Ignored by
    /// default because CI machines have no audio hardware; run locally with
    /// `cargo test -- --ignored real_device`.
    #[test]
    #[ignore]
    fn real_device_recording_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let ev = events.clone();
        let rec = Recorder::new(db.clone(), Arc::new(move |name, _| ev.lock().push(name.to_string())));
        let status = rec
            .start(StartOptions {
                title: Some("Test".into()),
                resume_id: None,
                microphone_device_id: None,
                output_device_id: None,
                capture_system: true,
                system_audio_available: true,
                retention: "delete_after_processing".into(),
                meetings_root: root.path().to_path_buf(),
                speech: None,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let id = rec.stop().unwrap();
        assert_eq!(id, status.meeting_id);
        let tracks = db.with(|c| store::tracks(c, &id)).unwrap();
        for source in [AudioSource::Microphone, AudioSource::System] {
            assert!(tracks.iter().any(|t| t.source == source), "missing {source:?} track");
        }
        for t in &tracks {
            let ms = crate::audio::wav::duration_ms(std::path::Path::new(&t.path)).unwrap();
            eprintln!("{:?}: recorded {ms} ms", t.source);
            assert!(ms > 1000, "{:?} only {ms} ms", t.source);
        }
        assert!(events.lock().iter().any(|e| e == crate::events::AUDIO_LEVEL));
    }
}
