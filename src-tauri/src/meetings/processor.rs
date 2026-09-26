//! Post-meeting processing queue.
//!
//! ```text
//! stop ─► wait for live transcriber to drain
//!      ─► transcribe audio the live path did not cover (or everything)
//!      ─► speaker refinement (diarization + recognition)
//!      ─► mark transcript final, meeting ready
//!      ─► apply audio retention
//!      ─► release speech models (unless another meeting is recording)
//! ```
//!
//! One meeting is processed at a time on a dedicated thread. Every step is
//! persisted, so a crash or failure never loses the transcript or audio; the
//! meeting can simply be processed again.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use parking_lot::RwLock;
use rusqlite::params;
use serde::Serialize;
use ts_rs::TS;

use super::recorder::EventSink;
use super::store::{self, AudioTrackRow};
use super::transcript::{Coverage, LiveReport, LiveTranscriber, TranscribeCtx};
use super::MeetingStatus;
use crate::speech::chunking::ChunkConfig;
use crate::speech::engine::SpeechEngine;
use crate::speech::vad::{SpeechDetector, VadConfig};
use crate::speech::{ms_to_samples, SAMPLE_RATE};
use crate::storage::settings::AppSettings;
use crate::storage::{now, Database};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ProcessingStage {
    Queued,
    Transcribing,
    Speakers,
    Finishing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProcessingProgress {
    pub meeting_id: String,
    pub stage: ProcessingStage,
    /// 0..1 within the stage, when known.
    pub fraction: Option<f32>,
    pub message: Option<String>,
}

/// Additional processing steps (speaker refinement) plugged in by later
/// layers. Each step must be idempotent.
pub trait PostTranscriptStep: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, meeting_id: &str, report: &dyn Fn(Option<f32>)) -> anyhow::Result<()>;
}

pub struct ProcessorDeps {
    pub db: Database,
    pub engine: Arc<SpeechEngine>,
    pub sink: EventSink,
    pub settings: Arc<RwLock<AppSettings>>,
    pub is_recording: Arc<dyn Fn() -> bool + Send + Sync>,
    pub speaker_step: Option<Arc<dyn PostTranscriptStep>>,
    /// Called after a meeting becomes ready (e.g. to queue analysis).
    pub on_ready: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

pub struct Job {
    pub meeting_id: String,
    pub live: Option<LiveTranscriber>,
}

pub struct Processor {
    tx: Sender<Job>,
}

/// Gaps shorter than this are not re-transcribed (block rounding).
const MIN_GAP_SAMPLES: u64 = SAMPLE_RATE as u64 / 2;

impl Processor {
    pub fn start(deps: ProcessorDeps) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("meeting-processor".into())
            .spawn(move || run(deps, rx))
            .expect("spawn processor");
        Processor { tx }
    }

    pub fn enqueue(&self, job: Job) {
        if self.tx.send(job).is_err() {
            tracing::error!("processing queue is gone");
        }
    }
}

fn progress(deps: &ProcessorDeps, id: &str, stage: ProcessingStage, fraction: Option<f32>, message: Option<String>) {
    let p = ProcessingProgress { meeting_id: id.to_string(), stage, fraction, message };
    (deps.sink)(crate::events::PROCESSING_PROGRESS, serde_json::to_value(&p).unwrap_or_default());
}

fn set_error(db: &Database, id: &str, error: Option<&str>) {
    let _ = db.with(|c| {
        c.execute("UPDATE meetings SET processing_error = ?2, updated_at = ?3 WHERE id = ?1", params![id, error, now()])
    });
}

fn run(deps: ProcessorDeps, rx: Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        let id = job.meeting_id.clone();
        progress(&deps, &id, ProcessingStage::Transcribing, None, None);
        match process(&deps, job) {
            Ok(()) => {
                set_error(&deps.db, &id, None);
                let _ = store::set_status(&deps.db, &id, MeetingStatus::Ready);
                progress(&deps, &id, ProcessingStage::Done, Some(1.0), None);
                apply_retention_for(&deps.db, &id);
                if let Some(cb) = &deps.on_ready {
                    cb(&id);
                }
            }
            Err(e) => {
                tracing::error!(meeting_id = id, error = ?e, "processing failed");
                let msg = e.to_string();
                set_error(&deps.db, &id, Some(&msg));
                let _ = store::set_status(&deps.db, &id, MeetingStatus::Failed);
                progress(&deps, &id, ProcessingStage::Failed, None, Some(msg));
            }
        }
        if !(deps.is_recording)() {
            deps.engine.unload_asr();
        }
    }
}

fn process(deps: &ProcessorDeps, job: Job) -> anyhow::Result<()> {
    let id = job.meeting_id;
    // 1. Let the live transcriber finish everything it received.
    let report: LiveReport = match job.live {
        Some(live) => live.handle.join().unwrap_or_default(),
        None => LiveReport::default(),
    };
    if let Some(e) = &report.error {
        tracing::warn!(meeting_id = id, error = e, "live transcription had an error; falling back to full pass");
    }

    // 2. Transcribe whatever the live path did not cover.
    let tracks = deps.db.with(|c| store::tracks(c, &id))?;
    let usable: Vec<&AudioTrackRow> = tracks.iter().filter(|t| std::path::Path::new(&t.path).exists()).collect();
    let plan: Vec<(&AudioTrackRow, Vec<(u64, u64)>)> = usable
        .iter()
        .map(|t| {
            let start = ms_to_samples(t.start_offset_ms);
            let dur = crate::audio::wav::duration_ms(std::path::Path::new(&t.path)).unwrap_or(0);
            let end = start + ms_to_samples(dur);
            let empty = Coverage::default();
            let cov = report.coverage.get(&t.source).unwrap_or(&empty);
            (*t, cov.gaps(start, end, MIN_GAP_SAMPLES))
        })
        .filter(|(_, gaps)| !gaps.is_empty())
        .collect();
    let total: u64 = plan.iter().flat_map(|(_, g)| g.iter().map(|(s, e)| e - s)).sum();

    if total > 0 {
        if !deps.engine.asr_installed() {
            anyhow::bail!("The transcription models are not installed. Download them in Settings → Models, then choose “Transcribe again”.");
        }
        let settings = deps.settings.read().clone();
        let ctx = transcribe_ctx(deps, &id, &settings)?;
        let mut done = 0u64;
        for (track, gaps) in plan {
            for (gs, ge) in gaps {
                let track_start = ms_to_samples(track.start_offset_ms);
                let rel_start_ms = (gs - track_start) * 1000 / SAMPLE_RATE as u64;
                let rel_end_ms = (ge - track_start) * 1000 / SAMPLE_RATE as u64;
                let mut vad = SpeechDetector::new(&ctx.vad_model, &ctx.vad)?;
                let mut pos = gs;
                let mut err: Option<anyhow::Error> = None;
                crate::audio::wav::stream_16k_mono(std::path::Path::new(&track.path), rel_start_ms, Some(rel_end_ms), |block| {
                    for span in vad.accept(pos, block) {
                        if let Err(e) = ctx.transcribe_span(track.source, span, |_| {}) {
                            err = Some(e);
                            return false;
                        }
                    }
                    pos += block.len() as u64;
                    done += block.len() as u64;
                    progress(deps, &id, ProcessingStage::Transcribing, Some(done as f32 / total as f32), None);
                    true
                })?;
                if let Some(e) = err {
                    return Err(e);
                }
                for span in vad.flush() {
                    ctx.transcribe_span(track.source, span, |_| {})?;
                }
            }
        }
    }

    // 3. Speaker refinement.
    if let Some(step) = &deps.speaker_step {
        progress(deps, &id, ProcessingStage::Speakers, None, None);
        let report_fn = |f: Option<f32>| progress(deps, &id, ProcessingStage::Speakers, f, None);
        if let Err(e) = step.run(&id, &report_fn) {
            // Speaker labels are an enhancement; never fail the transcript for them.
            tracing::warn!(meeting_id = id, step = step.name(), error = ?e, "speaker step failed");
        }
    }

    // 4. Finalise.
    progress(deps, &id, ProcessingStage::Finishing, None, None);
    deps.db.with(|c| c.execute("UPDATE transcript_segments SET is_provisional = 0 WHERE meeting_id = ?1", [&id]))?;
    Ok(())
}

pub fn transcribe_ctx(deps: &ProcessorDeps, meeting_id: &str, settings: &AppSettings) -> anyhow::Result<TranscribeCtx> {
    let threads = settings.advanced.asr_threads.unwrap_or_else(default_asr_threads);
    let asr = deps.engine.asr(threads)?;
    let vad_model = deps.engine.vad_model().ok_or_else(|| anyhow::anyhow!("The speech detection model is not installed."))?;
    let (self_person_id, self_label) = self_identity(&deps.db, settings);
    Ok(TranscribeCtx {
        meeting_id: meeting_id.to_string(),
        db: deps.db.clone(),
        asr,
        vad_model,
        vad: VadConfig::default(),
        chunk: ChunkConfig::from_target(settings.advanced.asr_chunk_seconds),
        self_person_id,
        self_label,
    })
}

pub fn default_asr_threads() -> u32 {
    let n = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    (n / 2).clamp(1, 4)
}

/// The person using this computer, if they have identified themselves.
pub fn self_identity(db: &Database, settings: &AppSettings) -> (Option<String>, String) {
    let person: Option<(String, String)> = db
        .with(|c| {
            use rusqlite::OptionalExtension;
            match &settings.general.self_person_id {
                Some(id) => c
                    .query_row("SELECT id, display_name FROM people WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional(),
                None => c
                    .query_row("SELECT id, display_name FROM people WHERE is_self = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional(),
            }
        })
        .ok()
        .flatten();
    match person {
        Some((id, name)) => (Some(id), name),
        None => (None, "You".into()),
    }
}

/// Delete audio according to the meeting's retention policy. Only called
/// after processing has completed successfully.
pub fn apply_retention_for(db: &Database, meeting_id: &str) {
    let row: Option<(String, Option<String>, Option<String>)> = db
        .with(|c| {
            use rusqlite::OptionalExtension;
            c.query_row(
                "SELECT audio_retention, ended_at, audio_dir FROM meetings WHERE id = ?1 AND audio_deleted_at IS NULL AND status = 'ready'",
                [meeting_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })
        .ok()
        .flatten();
    let Some((retention, ended_at, Some(dir))) = row else { return };
    let due = match retention.as_str() {
        "delete_after_processing" => true,
        "keep_7_days" => ended_at
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(&e).ok())
            .is_some_and(|e| chrono::Utc::now().signed_duration_since(e) > chrono::Duration::days(7)),
        _ => false,
    };
    if !due {
        return;
    }
    let dir = std::path::PathBuf::from(dir);
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(meeting_id, error = %e, "could not delete meeting audio");
            return;
        }
    }
    let _ = db.with(|c| c.execute("UPDATE meetings SET audio_deleted_at = ?2 WHERE id = ?1", params![meeting_id, now()]));
    tracing::info!(meeting_id, "meeting audio deleted per retention policy");
}

/// Startup sweep for time-based retention.
pub fn apply_retention_all(db: &Database) {
    let ids: Vec<String> = db
        .with(|c| {
            let mut stmt = c.prepare("SELECT id FROM meetings WHERE audio_deleted_at IS NULL AND status = 'ready' AND audio_retention != 'keep_forever'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .unwrap_or_default();
    for id in ids {
        apply_retention_for(db, &id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_deletes_only_when_due() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (keep, _, keep_dir) = store::create_meeting(&db, "a", root.path(), true, true, "keep_forever").unwrap();
        let (del, _, del_dir) = store::create_meeting(&db, "b", root.path(), true, true, "delete_after_processing").unwrap();
        let (week, _, week_dir) = store::create_meeting(&db, "c", root.path(), true, true, "keep_7_days").unwrap();
        for d in [&keep_dir, &del_dir, &week_dir] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("microphone-0.wav"), b"x").unwrap();
        }
        for id in [&keep, &del, &week] {
            store::finish_recording(&db, id, 1000, MeetingStatus::Ready).unwrap();
        }
        apply_retention_all(&db);
        assert!(keep_dir.exists());
        assert!(!del_dir.exists());
        assert!(week_dir.exists(), "not yet 7 days old");
        db.with(|c| c.execute("UPDATE meetings SET ended_at = '2020-01-01T00:00:00Z' WHERE id = ?1", [&week])).unwrap();
        apply_retention_all(&db);
        assert!(!week_dir.exists());
    }

    #[test]
    fn processing_never_deletes_audio_of_failed_meetings() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, _, dir) = store::create_meeting(&db, "a", root.path(), true, true, "delete_after_processing").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        store::finish_recording(&db, &id, 1000, MeetingStatus::Failed).unwrap();
        apply_retention_for(&db, &id);
        assert!(dir.exists());
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::audio::capture::{AudioSource, TrackInfo};
    use crate::models::manager::ModelManager;

    /// Full post-meeting pass on real models over a synthetic 48 kHz stereo
    /// "meeting audio" track. `MINUTES_DATA_DIR=... cargo test full_pipeline -- --ignored`
    #[test]
    #[ignore]
    fn full_pipeline_transcribes_recorded_audio() {
        let data = std::path::PathBuf::from(std::env::var("MINUTES_DATA_DIR").unwrap());
        let models_db = Database::open(&data.join("minutes.db")).unwrap();
        let manager = Arc::new(ModelManager::new(models_db, data.join("models")));
        let engine = Arc::new(SpeechEngine::new(manager));
        assert!(engine.asr_installed());

        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, _, dir) = store::create_meeting(&db, "Pipeline", root.path(), true, true, "keep_forever").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        // 3 s silence, sample, 2 s silence, sample — as 48 kHz stereo.
        let speech = crate::audio::wav::read_16k_mono(&data.join("models/parakeet-tdt-0.6b-v3-int8/test_wavs/en.wav")).unwrap();
        let mut mono16 = vec![0.0f32; 48_000];
        mono16.extend_from_slice(&speech);
        mono16.extend(std::iter::repeat_n(0.0, 32_000));
        mono16.extend_from_slice(&speech);
        let path = dir.join("system-0.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for s in &mono16 {
            let v = (s * 32_000.0) as i16;
            for _ in 0..3 {
                w.write_sample(v).unwrap();
                w.write_sample(v).unwrap();
            }
        }
        w.finalize().unwrap();
        let info = TrackInfo { source: AudioSource::System, device_id: None, device_name: "Speakers".into(), sample_rate: 48_000, channels: 2, path, start_offset_ms: 0 };
        store::insert_track(&db, &id, &info, 0).unwrap();
        store::finish_recording(&db, &id, 12_000, MeetingStatus::Processing).unwrap();

        let events = Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
        let ev = events.clone();
        let processor = Processor::start(ProcessorDeps {
            db: db.clone(),
            engine,
            sink: Arc::new(move |_, v| ev.lock().push(v)),
            settings: Arc::new(RwLock::new(AppSettings::default())),
            is_recording: Arc::new(|| false),
            speaker_step: None,
            on_ready: None,
        });
        processor.enqueue(Job { meeting_id: id.clone(), live: None });
        for _ in 0..600 {
            if store::get_summary(&db, &id).unwrap().unwrap().status == MeetingStatus::Ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let segs = db.with(|c| crate::meetings::transcript::list_segments(c, &id)).unwrap();
        for s in &segs {
            println!("{} - {} {:?}", s.start_ms, s.end_ms, s.text);
        }
        assert_eq!(store::get_summary(&db, &id).unwrap().unwrap().status, MeetingStatus::Ready);
        assert_eq!(segs.len(), 2, "two utterances");
        assert!(segs.iter().all(|s| s.text.to_lowercase().contains("country") && !s.is_provisional));
        // Timeline: first utterance starts after ~3 s of silence.
        assert!(segs[0].start_ms >= 2_500 && segs[0].start_ms <= 3_800, "{}", segs[0].start_ms);
        assert!(segs[1].start_ms > segs[0].end_ms);
    }
}
