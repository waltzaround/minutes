//! Transcript storage and the live transcription worker.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::recorder::EventSink;
use crate::audio::capture::{AudioSource, SpeechBlock};
use crate::speech::chunking::{self, ChunkConfig};
use crate::speech::vad::{SpeechDetector, SpeechSpan, VadConfig};
use crate::speech::{samples_to_ms, SpeechRecognizer, Word};
use crate::storage::{new_id, now, Database};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SegmentSource {
    Microphone,
    System,
    Mixed,
}

impl From<AudioSource> for SegmentSource {
    fn from(s: AudioSource) -> Self {
        match s {
            AudioSource::Microphone => SegmentSource::Microphone,
            AudioSource::System => SegmentSource::System,
        }
    }
}

impl SegmentSource {
    pub fn as_db(self) -> &'static str {
        match self {
            SegmentSource::Microphone => "microphone",
            SegmentSource::System => "system",
            SegmentSource::Mixed => "mixed",
        }
    }
    pub fn from_db(s: &str) -> Self {
        match s {
            "microphone" => SegmentSource::Microphone,
            "system" => SegmentSource::System,
            _ => SegmentSource::Mixed,
        }
    }
}

/// A transcript segment. Text is independent of speaker attribution:
/// `speaker_cluster_id` / `person_id` can change after transcription
/// (diarization refinement, user corrections) without touching the text.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptSegment {
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub source: SegmentSource,
    pub speaker_cluster_id: Option<String>,
    pub person_id: Option<String>,
    pub speaker_confidence: Option<f32>,
    pub text: String,
    pub transcription_confidence: Option<f32>,
    pub words: Vec<Word>,
    pub is_provisional: bool,
    pub edited: bool,
}

/// Payload of the `transcript_segment` event during recording.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LiveSegment {
    pub meeting_id: String,
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub source: SegmentSource,
    pub speaker_label: String,
    pub text: String,
}

pub struct NewSegment<'a> {
    pub meeting_id: &'a str,
    pub source: SegmentSource,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: &'a str,
    pub words: &'a [Word],
    pub person_id: Option<&'a str>,
    pub speaker_confidence: Option<f32>,
}

pub fn insert_segment(db: &Database, s: &NewSegment<'_>) -> rusqlite::Result<String> {
    let id = new_id();
    let words = serde_json::to_string(s.words).unwrap_or_else(|_| "[]".into());
    db.with(|c| {
        c.execute(
            "INSERT INTO transcript_segments (id, meeting_id, start_ms, end_ms, source, person_id, speaker_confidence,
                                              text, original_text, words_json, is_provisional, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, 1, ?10)",
            params![
                id,
                s.meeting_id,
                s.start_ms as i64,
                s.end_ms.max(s.start_ms) as i64,
                s.source.as_db(),
                s.person_id,
                s.speaker_confidence,
                s.text,
                words,
                now()
            ],
        )
    })?;
    Ok(id)
}

pub fn list_segments(conn: &Connection, meeting_id: &str) -> rusqlite::Result<Vec<TranscriptSegment>> {
    let mut stmt = conn.prepare(
        "SELECT id, start_ms, end_ms, source, speaker_cluster_id, person_id, speaker_confidence, text,
                transcription_confidence, words_json, is_provisional, edited_at
         FROM transcript_segments WHERE meeting_id = ?1 ORDER BY start_ms, end_ms, id",
    )?;
    let rows = stmt.query_map([meeting_id], |r| {
        Ok(TranscriptSegment {
            id: r.get(0)?,
            start_ms: r.get::<_, i64>(1)? as u64,
            end_ms: r.get::<_, i64>(2)? as u64,
            source: SegmentSource::from_db(&r.get::<_, String>(3)?),
            speaker_cluster_id: r.get(4)?,
            person_id: r.get(5)?,
            speaker_confidence: r.get(6)?,
            text: r.get(7)?,
            transcription_confidence: r.get(8)?,
            words: r
                .get::<_, Option<String>>(9)?
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
            is_provisional: r.get::<_, i64>(10)? != 0,
            edited: r.get::<_, Option<String>>(11)?.is_some(),
        })
    })?;
    rows.collect()
}

/// Half-open ranges of 16 kHz samples, kept sorted and merged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Coverage(pub Vec<(u64, u64)>);

impl Coverage {
    pub fn add(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        if let Some(last) = self.0.last_mut() {
            if start <= last.1 && start >= last.0 {
                last.1 = last.1.max(end);
                return;
            }
        }
        self.0.push((start, end));
        self.0.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.0.len());
        for (s, e) in self.0.drain(..) {
            match merged.last_mut() {
                Some(l) if s <= l.1 => l.1 = l.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        self.0 = merged;
    }

    /// Parts of `[start, end)` not covered, ignoring gaps shorter than `min_gap`.
    pub fn gaps(&self, start: u64, end: u64, min_gap: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut pos = start;
        for &(s, e) in &self.0 {
            if e <= pos {
                continue;
            }
            if s >= end {
                break;
            }
            if s > pos && s - pos >= min_gap {
                out.push((pos, s.min(end)));
            }
            pos = pos.max(e);
        }
        if end > pos && end - pos >= min_gap {
            out.push((pos, end));
        }
        out
    }
}

#[derive(Debug, Default)]
pub struct LiveReport {
    pub coverage: HashMap<AudioSource, Coverage>,
    pub segments: usize,
    pub error: Option<String>,
}

pub struct SpeakerDefaults {
    /// Person using this computer; the microphone stream is theirs.
    pub self_person_id: Option<String>,
    pub self_label: String,
}

/// Everything a transcription pass needs; shared by the live worker and
/// the post-meeting processor.
#[derive(Clone)]
pub struct TranscribeCtx {
    pub meeting_id: String,
    pub db: Database,
    pub asr: Arc<dyn SpeechRecognizer>,
    pub vad_model: PathBuf,
    pub vad: VadConfig,
    pub chunk: ChunkConfig,
    pub self_person_id: Option<String>,
    pub self_label: String,
}

impl TranscribeCtx {
    /// Transcribe one VAD span (splitting long ones) and store the results.
    pub fn transcribe_span(&self, source: AudioSource, span: SpeechSpan, mut on_segment: impl FnMut(LiveSegment)) -> anyhow::Result<usize> {
        let mut n = 0;
        for chunk in chunking::split(span, self.chunk) {
            let result = self.asr.transcribe(&chunk.samples)?;
            if result.text.is_empty() {
                continue;
            }
            let base_ms = samples_to_ms(chunk.start_sample);
            let words: Vec<Word> = result
                .words
                .iter()
                .map(|w| Word { text: w.text.clone(), start_ms: base_ms + w.start_ms, end_ms: base_ms + w.end_ms })
                .collect();
            let start_ms = words.first().map(|w| w.start_ms).unwrap_or(base_ms);
            let end_ms = words
                .last()
                .map(|w| w.end_ms)
                .unwrap_or(base_ms + samples_to_ms(chunk.samples.len() as u64));
            let person = if source == AudioSource::Microphone { self.self_person_id.as_deref() } else { None };
            let id = insert_segment(
                &self.db,
                &NewSegment {
                    meeting_id: &self.meeting_id,
                    source: source.into(),
                    start_ms,
                    end_ms,
                    text: &result.text,
                    words: &words,
                    person_id: person,
                    // The local microphone is an isolated stream: its speaker
                    // is deterministic when the user has identified themself.
                    speaker_confidence: person.map(|_| 1.0),
                },
            )?;
            n += 1;
            on_segment(LiveSegment {
                meeting_id: self.meeting_id.clone(),
                id,
                start_ms,
                end_ms,
                source: source.into(),
                speaker_label: match source {
                    AudioSource::Microphone => self.self_label.clone(),
                    AudioSource::System => "Meeting audio".into(),
                },
                text: result.text,
            });
        }
        Ok(n)
    }
}

pub struct LiveTranscriber {
    pub handle: JoinHandle<LiveReport>,
}

/// Bounded queue into the live worker: 2 minutes of 0.5 s blocks per source.
pub const LIVE_QUEUE_BLOCKS: usize = 480;

/// Start the live worker. Speech blocks go through the returned sender; the
/// worker finishes (flushing VAD) when every sender has been dropped.
///
/// `make_ctx` runs on the worker thread (it loads the ASR model), so
/// recording starts immediately; blocks queue up meanwhile.
pub fn start_live(
    make_ctx: impl FnOnce() -> anyhow::Result<TranscribeCtx> + Send + 'static,
    sink: EventSink,
) -> (SyncSender<SpeechBlock>, LiveTranscriber) {
    let (tx, rx) = mpsc::sync_channel::<SpeechBlock>(LIVE_QUEUE_BLOCKS);
    let handle = std::thread::Builder::new()
        .name("live-transcriber".into())
        .spawn(move || match make_ctx() {
            Ok(ctx) => run_live(ctx, rx, sink),
            Err(e) => {
                tracing::warn!(error = %e, "live transcription unavailable");
                // Drain so capture never blocks; the processor covers everything later.
                while rx.recv().is_ok() {}
                LiveReport { error: Some(e.to_string()), ..Default::default() }
            }
        })
        .expect("spawn live transcriber");
    (tx, LiveTranscriber { handle })
}

fn run_live(ctx: TranscribeCtx, rx: Receiver<SpeechBlock>, sink: EventSink) -> LiveReport {
    let mut report = LiveReport::default();
    let mut detectors: HashMap<AudioSource, SpeechDetector> = HashMap::new();
    let mut failed = false;
    let emit = |seg: LiveSegment| sink(crate::events::TRANSCRIPT_SEGMENT, serde_json::to_value(&seg).unwrap_or_default());

    while let Ok(block) = rx.recv() {
        let cov = report.coverage.entry(block.source).or_default();
        if failed {
            continue; // processor will transcribe the uncovered audio
        }
        let det = match detectors.entry(block.source) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => match SpeechDetector::new(&ctx.vad_model, &ctx.vad) {
                Ok(d) => v.insert(d),
                Err(e) => {
                    report.error = Some(e.to_string());
                    failed = true;
                    continue;
                }
            },
        };
        cov.add(block.start_sample, block.start_sample + block.samples.len() as u64);
        for span in det.accept(block.start_sample, &block.samples) {
            match ctx.transcribe_span(block.source, span, &emit) {
                Ok(n) => report.segments += n,
                Err(e) => {
                    tracing::error!(error = %e, "live transcription failed; continuing after the meeting");
                    report.error = Some(e.to_string());
                    failed = true;
                    break;
                }
            }
        }
    }
    if !failed {
        for (source, det) in detectors.iter_mut() {
            for span in det.flush() {
                match ctx.transcribe_span(*source, span, &emit) {
                    Ok(n) => report.segments += n,
                    Err(e) => report.error = Some(e.to_string()),
                }
            }
        }
    } else {
        // Nothing we skipped counts as covered.
        report.coverage.clear();
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_merges_and_finds_gaps() {
        let mut c = Coverage::default();
        c.add(0, 100);
        c.add(100, 200);
        c.add(400, 500);
        c.add(150, 250);
        assert_eq!(c.0, vec![(0, 250), (400, 500)]);
        assert_eq!(c.gaps(0, 600, 10), vec![(250, 400), (500, 600)]);
        assert_eq!(c.gaps(0, 600, 150), vec![(250, 400)]);
        assert_eq!(Coverage::default().gaps(10, 20, 1), vec![(10, 20)]);
    }

    struct FakeAsr;
    impl SpeechRecognizer for FakeAsr {
        fn transcribe(&self, samples: &[f32]) -> anyhow::Result<crate::speech::Transcription> {
            Ok(crate::speech::Transcription {
                text: format!("{} samples", samples.len()),
                words: vec![Word { text: "x".into(), start_ms: 10, end_ms: 20 }],
            })
        }
    }

    #[test]
    fn segments_store_absolute_times_and_self_identity() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (mid, _, _) = super::super::store::create_meeting(&db, "t", root.path(), true, true, "keep_forever").unwrap();
        db.with(|c| c.execute("INSERT INTO people (id, display_name, is_self, created_at, updated_at) VALUES ('me','Walter',1,'t','t')", [])).unwrap();
        let ctx = TranscribeCtx {
            meeting_id: mid.clone(),
            db: db.clone(),
            asr: Arc::new(FakeAsr),
            vad_model: PathBuf::new(),
            vad: VadConfig::default(),
            chunk: ChunkConfig::from_target(20.0),
            self_person_id: Some("me".into()),
            self_label: "Walter".into(),
        };
        let span = SpeechSpan { start_sample: 16_000 * 60, samples: vec![0.1; 16_000] };
        let mut live = Vec::new();
        ctx.transcribe_span(AudioSource::Microphone, span, |s| live.push(s)).unwrap();
        let segs = db.with(|c| list_segments(c, &mid)).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].start_ms, 60_010);
        assert_eq!(segs[0].person_id.as_deref(), Some("me"));
        assert_eq!(live[0].speaker_label, "Walter");
        assert!(segs[0].is_provisional);
    }
}
