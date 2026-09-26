//! Speaker refinement after a meeting, and speaker editing.
//!
//! - The local microphone is an isolated stream: its speaker is the person
//!   using this computer (deterministic).
//! - Meeting (system) audio is diarized. For an in-person meeting recorded
//!   through a single room microphone (no system track), the microphone is
//!   diarized instead.
//! - Each diarization cluster gets a centroid voiceprint, matched against
//!   enrolled people with explicit known/possible/unknown confidence.
//! - Transcript words are aligned to diarization turns; a segment that spans
//!   a speaker change is split at the change.
//!
//! Text and speaker attribution are stored separately, so later corrections
//! never touch transcript text.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use rusqlite::params;
use serde::Serialize;
use ts_rs::TS;

use super::processor::{self_identity, PostTranscriptStep};
use super::store;
use super::transcript::{insert_segment, list_segments, NewSegment, SegmentSource, TranscriptSegment};
use crate::audio::capture::AudioSource;
use crate::people;
use crate::speech::diarization::{DiarizedTurn, Diarizer, NemoDiarizer};
use crate::speech::embeddings::{mean, EmbeddingModel, SherpaEmbedder, MIN_EMBEDDING_SAMPLES};
use crate::speech::engine::SpeechEngine;
use crate::speech::speaker_registry::{MatchThresholds, Registry, SpeakerIdentity};
use crate::speech::{ms_to_samples, Word};
use crate::storage::secrets::{SecretStore, VoiceprintCipher};
use crate::storage::settings::AppSettings;
use crate::storage::{new_id, now, Database};

/// Audio used for a cluster's voiceprint.
const CENTROID_AUDIO_MS: u64 = 30_000;
/// A speaker change inside a segment must last this long to split it.
const MIN_RUN_MS: u64 = 700;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeakerCluster {
    pub id: String,
    pub label: String,
    pub source: SegmentSource,
    pub person_id: Option<String>,
    pub identity: SpeakerIdentity,
    /// True once a person confirmed this identity.
    pub confirmed: bool,
    pub is_local_user: bool,
    pub segment_count: u32,
    pub speaking_ms: u64,
}

/// Assign each word the diarized speaker covering its midpoint (or the
/// nearest turn within 1 s), then collapse into runs. Short runs are merged
/// into their neighbours so a single mis-attributed word never splits a
/// segment.
pub fn align_words(words: &[Word], turns: &[DiarizedTurn]) -> Vec<(Option<u32>, std::ops::Range<usize>)> {
    let speaker_at = |ms: u64| -> Option<u32> {
        if let Some(t) = turns.iter().find(|t| t.start_ms <= ms && ms < t.end_ms) {
            return Some(t.speaker);
        }
        turns
            .iter()
            .map(|t| (t.speaker, if ms < t.start_ms { t.start_ms - ms } else { ms.saturating_sub(t.end_ms) }))
            .filter(|(_, d)| *d <= 1_000)
            .min_by_key(|(_, d)| *d)
            .map(|(s, _)| s)
    };
    let labels: Vec<Option<u32>> = words.iter().map(|w| speaker_at((w.start_ms + w.end_ms) / 2)).collect();
    let mut runs: Vec<(Option<u32>, std::ops::Range<usize>)> = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        match runs.last_mut() {
            Some((s, r)) if *s == *l || l.is_none() => r.end = i + 1,
            _ => runs.push((*l, i..i + 1)),
        }
    }
    // Merge short runs into the previous (or next) run.
    let dur = |r: &std::ops::Range<usize>| words[r.end - 1].end_ms.saturating_sub(words[r.start].start_ms);
    let mut merged: Vec<(Option<u32>, std::ops::Range<usize>)> = Vec::new();
    for (s, r) in runs {
        let short = dur(&r) < MIN_RUN_MS && r.len() < 3;
        match merged.last_mut() {
            Some((_, prev)) if short => prev.end = r.end,
            Some((ps, prev)) if *ps == s => prev.end = r.end,
            _ => merged.push((s, r)),
        }
    }
    if merged.len() > 1 {
        let (s0, r0) = merged[0].clone();
        if dur(&r0) < MIN_RUN_MS && r0.len() < 3 {
            merged.remove(0);
            merged[0].1.start = r0.start;
            let _ = s0;
        }
    }
    merged
}

pub struct SpeakerRefinement {
    pub db: Database,
    pub engine: Arc<SpeechEngine>,
    pub settings: Arc<RwLock<AppSettings>>,
    pub secrets: Arc<dyn SecretStore>,
}

impl PostTranscriptStep for SpeakerRefinement {
    fn name(&self) -> &'static str {
        "speakers"
    }

    fn run(&self, meeting_id: &str, report: &dyn Fn(Option<f32>)) -> anyhow::Result<()> {
        let settings = self.settings.read().clone();
        // Respect manual edits: never overwrite confirmed identities.
        let confirmed: i64 = self.db.with(|c| {
            c.query_row(
                "SELECT count(*) FROM speaker_clusters WHERE meeting_id = ?1 AND identity_type = 'confirmed'",
                [meeting_id],
                |r| r.get(0),
            )
        })?;
        if confirmed > 0 {
            return Ok(());
        }
        self.db.with(|c| c.execute("DELETE FROM speaker_clusters WHERE meeting_id = ?1", [meeting_id]))?;

        let tracks = self.db.with(|c| store::tracks(c, meeting_id))?;
        let has_system = tracks.iter().any(|t| t.source == AudioSource::System && t.frames_written > 0);
        let (self_person, self_label) = self_identity(&self.db, &settings);

        // 1. Local microphone → the person at this computer.
        let has_mic = tracks.iter().any(|t| t.source == AudioSource::Microphone);
        if has_system && has_mic {
            let cid = insert_cluster(&self.db, meeting_id, &self_label, SegmentSource::Microphone, self_person.as_deref(), "local_user", self_person.as_ref().map(|_| 1.0))?;
            self.db.with(|c| {
                c.execute(
                    "UPDATE transcript_segments SET speaker_cluster_id = ?2, person_id = ?3, speaker_confidence = ?4
                     WHERE meeting_id = ?1 AND source = 'microphone'",
                    params![meeting_id, cid, self_person, self_person.as_ref().map(|_| 1.0f32)],
                )
            })?;
        }

        // 2. Diarize the shared stream.
        let target = if has_system { AudioSource::System } else { AudioSource::Microphone };
        let (Some(diar_model), Some(emb_model)) = (self.engine.diarization_model(), self.engine.embedding_model()) else {
            return self.single_cluster_fallback(meeting_id, target, "Speaker models are not installed");
        };
        let audio = assemble_track_audio(&tracks, target)?;
        if audio.len() < 16_000 * 2 {
            return self.single_cluster_fallback(meeting_id, target, "too little audio");
        }
        report(Some(0.1));
        let threads = crate::meetings::processor::default_asr_threads();
        let (runtime, work_dir) = self.engine.diarization_runtime();
        let diarizer = match NemoDiarizer::new(&runtime, &diar_model, settings.advanced.inference_backend, &work_dir) {
            Ok(d) => d,
            Err(e) => return self.single_cluster_fallback(meeting_id, target, &e.to_string()),
        };
        let turns = diarizer.diarize(&audio)?;
        report(Some(0.6));
        if turns.is_empty() {
            return self.single_cluster_fallback(meeting_id, target, "no speech turns");
        }

        // 3. Voiceprint per cluster and identity matching.
        let embedder = SherpaEmbedder::load(&emb_model, threads)?;
        let cipher = VoiceprintCipher::load_or_create(self.secrets.as_ref())?;
        let mut registry = Registry::default();
        for (person, prints) in people::load_voiceprints(&self.db, &cipher)? {
            // With a separate microphone stream the local user is not in the meeting audio.
            if has_system && Some(&person) == self_person.as_ref() {
                continue;
            }
            registry.add(&person, prints);
        }
        let thresholds = MatchThresholds {
            known: settings.advanced.speaker_known_threshold,
            possible: settings.advanced.speaker_possible_threshold,
            margin: 0.08,
        };
        let mut order: Vec<u32> = Vec::new();
        for t in &turns {
            if !order.contains(&t.speaker) {
                order.push(t.speaker);
            }
        }
        let mut centroids: HashMap<u32, Vec<f32>> = HashMap::new();
        for spk in &order {
            if let Some(c) = cluster_centroid(&audio, &turns, *spk, &embedder) {
                centroids.insert(*spk, c);
            }
        }
        // Greedy assignment: most confident cluster claims its person first.
        let mut identities: HashMap<u32, SpeakerIdentity> = HashMap::new();
        if !registry.is_empty() {
            let mut scored: Vec<(u32, f32)> = centroids
                .iter()
                .map(|(s, c)| (*s, registry.rank(c).first().map(|r| r.1).unwrap_or(0.0)))
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let mut taken: Vec<String> = Vec::new();
            for (spk, _) in scored {
                let id = registry.identify(&centroids[&spk], thresholds, &taken);
                if let SpeakerIdentity::Known { person_id, .. } = &id {
                    taken.push(person_id.clone());
                }
                identities.insert(spk, id);
            }
        }
        report(Some(0.8));

        let mut cluster_ids: HashMap<u32, String> = HashMap::new();
        for (n, spk) in order.iter().enumerate() {
            let ident = identities.get(spk).cloned().unwrap_or(SpeakerIdentity::Unknown);
            let (person, kind, conf) = match &ident {
                SpeakerIdentity::Known { person_id, confidence } => (Some(person_id.as_str()), "known", Some(*confidence)),
                SpeakerIdentity::Possible { person_id, confidence } => (Some(person_id.as_str()), "possible", Some(*confidence)),
                SpeakerIdentity::Unknown => (None, "unknown", None),
            };
            let id = insert_cluster(&self.db, meeting_id, &format!("Speaker {}", n + 1), target.into(), person, kind, conf)?;
            cluster_ids.insert(*spk, id);
        }

        // 4. Align transcript segments of the diarized source.
        let segments: Vec<TranscriptSegment> = self
            .db
            .with(|c| list_segments(c, meeting_id))?
            .into_iter()
            .filter(|s| s.source == SegmentSource::from(target))
            .collect();
        let person_for = |spk: Option<u32>| -> (Option<String>, Option<String>, Option<f32>) {
            let Some(spk) = spk else { return (None, None, None) };
            let cid = cluster_ids.get(&spk).cloned();
            match identities.get(&spk) {
                // Only confident matches attribute text to a person; "possible"
                // stays on the cluster until the user confirms it.
                Some(SpeakerIdentity::Known { person_id, confidence }) => (cid, Some(person_id.clone()), Some(*confidence)),
                Some(SpeakerIdentity::Possible { confidence, .. }) => (cid, None, Some(*confidence)),
                _ => (cid, None, None),
            }
        };
        for seg in segments {
            let runs = if seg.words.is_empty() {
                let mid = (seg.start_ms + seg.end_ms) / 2;
                let w = [Word { text: seg.text.clone(), start_ms: mid, end_ms: mid }];
                align_words(&w, &turns).into_iter().map(|(s, _)| (s, 0..0)).collect::<Vec<_>>()
            } else {
                align_words(&seg.words, &turns)
            };
            if runs.len() <= 1 {
                let (cid, person, conf) = person_for(runs.first().and_then(|r| r.0));
                self.db.with(|c| {
                    c.execute(
                        "UPDATE transcript_segments SET speaker_cluster_id = ?2, person_id = ?3, speaker_confidence = ?4 WHERE id = ?1",
                        params![seg.id, cid, person, conf],
                    )
                })?;
                continue;
            }
            // Split at speaker changes.
            self.db.transaction(|tx| {
                tx.execute("DELETE FROM transcript_segments WHERE id = ?1", [&seg.id])?;
                Ok(())
            })?;
            for (spk, range) in runs {
                let words = &seg.words[range];
                let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                let (cid, person, conf) = person_for(spk);
                let new_id = insert_segment(
                    &self.db,
                    &NewSegment {
                        meeting_id,
                        source: seg.source,
                        start_ms: words.first().map(|w| w.start_ms).unwrap_or(seg.start_ms),
                        end_ms: words.last().map(|w| w.end_ms).unwrap_or(seg.end_ms),
                        text: &text,
                        words,
                        person_id: person.as_deref(),
                        speaker_confidence: conf,
                    },
                )?;
                self.db.with(|c| c.execute("UPDATE transcript_segments SET speaker_cluster_id = ?2 WHERE id = ?1", params![new_id, cid]))?;
            }
        }
        report(Some(1.0));
        Ok(())
    }
}

impl SpeakerRefinement {
    /// Without diarization everyone on the stream shares one cluster.
    fn single_cluster_fallback(&self, meeting_id: &str, source: AudioSource, reason: &str) -> anyhow::Result<()> {
        tracing::info!(meeting_id, reason, "speaker separation skipped");
        let label = if source == AudioSource::System { "Meeting audio" } else { "Room" };
        let cid = insert_cluster(&self.db, meeting_id, label, source.into(), None, "unknown", None)?;
        self.db.with(|c| {
            c.execute(
                "UPDATE transcript_segments SET speaker_cluster_id = ?2 WHERE meeting_id = ?1 AND source = ?3 AND speaker_cluster_id IS NULL",
                params![meeting_id, cid, source.as_str()],
            )
        })?;
        Ok(())
    }
}

fn insert_cluster(
    db: &Database,
    meeting_id: &str,
    label: &str,
    source: SegmentSource,
    person_id: Option<&str>,
    identity: &str,
    confidence: Option<f32>,
) -> rusqlite::Result<String> {
    let id = new_id();
    let ts = now();
    db.with(|c| {
        c.execute(
            "INSERT INTO speaker_clusters (id, meeting_id, label, source, person_id, identity_type, identity_confidence, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![id, meeting_id, label, source.as_db(), person_id, identity, confidence, ts],
        )
    })?;
    Ok(id)
}

/// Build one 16 kHz buffer for a source on the meeting timeline (segments
/// placed at their offsets; gaps are silence).
pub fn assemble_track_audio(tracks: &[store::AudioTrackRow], source: AudioSource) -> anyhow::Result<Vec<f32>> {
    let mut out: Vec<f32> = Vec::new();
    for t in tracks.iter().filter(|t| t.source == source) {
        let path = std::path::Path::new(&t.path);
        if !path.exists() {
            continue;
        }
        let offset = ms_to_samples(t.start_offset_ms) as usize;
        if out.len() < offset {
            out.resize(offset, 0.0);
        }
        let mut pos = offset;
        crate::audio::wav::stream_16k_mono(path, 0, None, |block| {
            let end = pos + block.len();
            if out.len() < end {
                out.resize(end, 0.0);
            }
            out[pos..end].copy_from_slice(block);
            pos = end;
            true
        })?;
    }
    Ok(out)
}

fn cluster_centroid(audio: &[f32], turns: &[DiarizedTurn], speaker: u32, embedder: &dyn EmbeddingModel) -> Option<Vec<f32>> {
    let mut mine: Vec<&DiarizedTurn> = turns.iter().filter(|t| t.speaker == speaker).collect();
    mine.sort_by_key(|t| std::cmp::Reverse(t.end_ms - t.start_ms));
    let mut used = 0u64;
    let mut embs = Vec::new();
    for t in mine {
        if used >= CENTROID_AUDIO_MS {
            break;
        }
        let s = ms_to_samples(t.start_ms) as usize;
        let e = (ms_to_samples(t.end_ms) as usize).min(audio.len());
        if e <= s || e - s < MIN_EMBEDDING_SAMPLES {
            continue;
        }
        if let Ok(v) = embedder.embed(&audio[s..e]) {
            embs.push(v);
            used += t.end_ms - t.start_ms;
        }
    }
    mean(&embs)
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

pub fn list_clusters(db: &Database, meeting_id: &str) -> rusqlite::Result<Vec<SpeakerCluster>> {
    db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT sc.id, sc.label, sc.source, sc.person_id, sc.identity_type, sc.identity_confidence,
                    (SELECT count(*) FROM transcript_segments t WHERE t.speaker_cluster_id = sc.id),
                    (SELECT COALESCE(sum(t.end_ms - t.start_ms), 0) FROM transcript_segments t WHERE t.speaker_cluster_id = sc.id)
             FROM speaker_clusters sc WHERE sc.meeting_id = ?1 AND sc.merged_into IS NULL
             ORDER BY sc.source = 'microphone' DESC, sc.created_at",
        )?;
        let rows = stmt.query_map([meeting_id], |r| {
            let kind: String = r.get(4)?;
            let person: Option<String> = r.get(3)?;
            let conf: Option<f32> = r.get(5)?;
            let identity = match (kind.as_str(), &person) {
                ("known" | "confirmed" | "local_user", Some(p)) => SpeakerIdentity::Known { person_id: p.clone(), confidence: conf.unwrap_or(1.0) },
                ("possible", Some(p)) => SpeakerIdentity::Possible { person_id: p.clone(), confidence: conf.unwrap_or(0.0) },
                _ => SpeakerIdentity::Unknown,
            };
            Ok(SpeakerCluster {
                id: r.get(0)?,
                label: r.get(1)?,
                source: SegmentSource::from_db(&r.get::<_, String>(2)?),
                person_id: person,
                identity,
                confirmed: kind == "confirmed",
                is_local_user: kind == "local_user",
                segment_count: r.get(6)?,
                speaking_ms: r.get::<_, i64>(7)? as u64,
            })
        })?;
        rows.collect()
    })
}

fn bump_revision(tx: &rusqlite::Transaction<'_>, meeting_id: &str) -> rusqlite::Result<()> {
    tx.execute("UPDATE meetings SET transcript_revision = transcript_revision + 1, updated_at = ?2 WHERE id = ?1", params![meeting_id, now()])?;
    Ok(())
}

fn cluster_meeting(tx: &rusqlite::Transaction<'_>, cluster_id: &str) -> rusqlite::Result<String> {
    tx.query_row("SELECT meeting_id FROM speaker_clusters WHERE id = ?1", [cluster_id], |r| r.get(0))
}

pub fn rename_cluster(db: &Database, cluster_id: &str, label: &str) -> rusqlite::Result<()> {
    db.transaction(|tx| {
        tx.execute("UPDATE speaker_clusters SET label = ?2, updated_at = ?3 WHERE id = ?1", params![cluster_id, label.trim(), now()])?;
        let m = cluster_meeting(tx, cluster_id)?;
        bump_revision(tx, &m)
    })
}

/// Confirm (or clear) the person for a cluster. Confirmed identities are
/// never overwritten by automatic refinement.
pub fn set_cluster_person(db: &Database, cluster_id: &str, person_id: Option<&str>) -> rusqlite::Result<()> {
    db.transaction(|tx| {
        let kind = if person_id.is_some() { "confirmed" } else { "unknown" };
        tx.execute(
            "UPDATE speaker_clusters SET person_id = ?2, identity_type = ?3, identity_confidence = ?4, updated_at = ?5 WHERE id = ?1",
            params![cluster_id, person_id, kind, person_id.map(|_| 1.0f32), now()],
        )?;
        tx.execute(
            "UPDATE transcript_segments SET person_id = ?2, speaker_confidence = ?3 WHERE speaker_cluster_id = ?1",
            params![cluster_id, person_id, person_id.map(|_| 1.0f32)],
        )?;
        if let Some(p) = person_id {
            let m = cluster_meeting(tx, cluster_id)?;
            tx.execute("INSERT OR IGNORE INTO meeting_participants (meeting_id, person_id) VALUES (?1, ?2)", params![m, p])?;
        }
        let m = cluster_meeting(tx, cluster_id)?;
        bump_revision(tx, &m)
    })
}

/// Merge `from` into `into` (same meeting).
pub fn merge_clusters(db: &Database, from: &str, into: &str) -> anyhow::Result<()> {
    anyhow::ensure!(from != into, "Choose two different speakers.");
    db.transaction(|tx| {
        let m1 = cluster_meeting(tx, from)?;
        let m2 = cluster_meeting(tx, into)?;
        if m1 != m2 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let (person, conf): (Option<String>, Option<f32>) =
            tx.query_row("SELECT person_id, identity_confidence FROM speaker_clusters WHERE id = ?1", [into], |r| Ok((r.get(0)?, r.get(1)?)))?;
        tx.execute(
            "UPDATE transcript_segments SET speaker_cluster_id = ?2, person_id = ?3, speaker_confidence = ?4 WHERE speaker_cluster_id = ?1",
            params![from, into, person, conf],
        )?;
        tx.execute("UPDATE speaker_clusters SET merged_into = ?2, updated_at = ?3 WHERE id = ?1", params![from, into, now()])?;
        bump_revision(tx, &m1)
    })?;
    Ok(())
}

/// Split a segment at a word index; the second part is assigned to
/// `cluster_id` (or a new "Speaker N" cluster when `None`).
pub fn split_segment(db: &Database, segment_id: &str, at_word: usize, cluster_id: Option<&str>) -> anyhow::Result<()> {
    let (meeting_id, words_json, source, cluster, start, end, text): (String, Option<String>, String, Option<String>, i64, i64, String) = db.with(|c| {
        c.query_row(
            "SELECT meeting_id, words_json, source, speaker_cluster_id, start_ms, end_ms, text FROM transcript_segments WHERE id = ?1",
            [segment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
    })?;
    let words: Vec<Word> = words_json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    anyhow::ensure!(at_word > 0 && at_word < words.len(), "Choose a point inside the segment to split.");
    let _ = (start, end, text);
    let target = match cluster_id {
        Some(c) => c.to_string(),
        None => {
            let n: i64 = db.with(|c| c.query_row("SELECT count(*) FROM speaker_clusters WHERE meeting_id = ?1", [&meeting_id], |r| r.get(0)))?;
            insert_cluster(db, &meeting_id, &format!("Speaker {}", n + 1), SegmentSource::from_db(&source), None, "unknown", None)?
        }
    };
    let (first, second) = words.split_at(at_word);
    let join = |w: &[Word]| w.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" ");
    db.transaction(|tx| {
        let (person, conf): (Option<String>, Option<f32>) = tx.query_row(
            "SELECT person_id, identity_confidence FROM speaker_clusters WHERE id = ?1",
            [&target],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.execute(
            "UPDATE transcript_segments SET text = ?2, words_json = ?3, end_ms = ?4, edited_at = ?5 WHERE id = ?1",
            params![segment_id, join(first), serde_json::to_string(first).unwrap_or_default(), first.last().map(|w| w.end_ms as i64), now()],
        )?;
        tx.execute(
            "INSERT INTO transcript_segments (id, meeting_id, start_ms, end_ms, source, speaker_cluster_id, person_id, speaker_confidence,
                                              text, original_text, words_json, is_provisional, edited_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, 0, ?11, ?11)",
            params![
                new_id(),
                meeting_id,
                second.first().map(|w| w.start_ms as i64),
                second.last().map(|w| w.end_ms as i64),
                source,
                target,
                person,
                conf,
                join(second),
                serde_json::to_string(second).unwrap_or_default(),
                now()
            ],
        )?;
        let _ = cluster;
        bump_revision(tx, &meeting_id)
    })?;
    Ok(())
}

/// Correct segment text. The original ASR text is preserved.
pub fn edit_segment_text(db: &Database, segment_id: &str, text: &str) -> anyhow::Result<()> {
    let text = text.trim();
    anyhow::ensure!(!text.is_empty(), "A segment can't be empty. Delete it instead.");
    db.transaction(|tx| {
        let m: String = tx.query_row("SELECT meeting_id FROM transcript_segments WHERE id = ?1", [segment_id], |r| r.get(0))?;
        tx.execute(
            "UPDATE transcript_segments SET text = ?2, edited_at = ?3 WHERE id = ?1",
            params![segment_id, text, now()],
        )?;
        bump_revision(tx, &m)
    })?;
    Ok(())
}

/// Audio of a cluster's longest segments, for improving a voice profile
/// after the user confirmed who it is. Returns nothing if audio was deleted.
pub fn confirmed_speech_embeddings(
    db: &Database,
    engine: &SpeechEngine,
    cluster_id: &str,
) -> anyhow::Result<(String, Vec<Vec<f32>>)> {
    let meeting_id: String = db.with(|c| c.query_row("SELECT meeting_id FROM speaker_clusters WHERE id = ?1", [cluster_id], |r| r.get(0)))?;
    let segs: Vec<(u64, u64, String)> = db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT start_ms, end_ms, source FROM transcript_segments WHERE speaker_cluster_id = ?1
             ORDER BY (end_ms - start_ms) DESC LIMIT 6",
        )?;
        let rows = stmt.query_map([cluster_id], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get(2)?)))?;
        rows.collect()
    })?;
    let tracks = db.with(|c| store::tracks(c, &meeting_id))?;
    let model = engine.embedding_model().ok_or_else(|| anyhow::anyhow!("The voice recognition model is not installed."))?;
    let embedder = SherpaEmbedder::load(&model, 2)?;
    let mut out = Vec::new();
    for (s, e, source) in segs {
        if e - s < 3_000 {
            continue;
        }
        let src = if source == "microphone" { AudioSource::Microphone } else { AudioSource::System };
        let Some(t) = tracks.iter().find(|t| t.source == src && t.start_offset_ms <= s && std::path::Path::new(&t.path).exists()) else { continue };
        let mut buf = Vec::new();
        crate::audio::wav::stream_16k_mono(std::path::Path::new(&t.path), s - t.start_offset_ms, Some(e - t.start_offset_ms), |b| {
            buf.extend_from_slice(b);
            true
        })?;
        if let Ok(v) = embedder.embed(&buf) {
            out.push(v);
        }
        if out.len() >= 3 {
            break;
        }
    }
    Ok((meeting_id, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, s: u64, e: u64) -> Word {
        Word { text: text.into(), start_ms: s, end_ms: e }
    }

    fn turn(s: u64, e: u64, spk: u32) -> DiarizedTurn {
        DiarizedTurn { start_ms: s, end_ms: e, speaker: spk }
    }

    #[test]
    fn single_speaker_is_one_run() {
        let words = vec![w("a", 0, 300), w("b", 300, 600), w("c", 600, 900)];
        let runs = align_words(&words, &[turn(0, 1000, 0)]);
        assert_eq!(runs, vec![(Some(0), 0..3)]);
    }

    #[test]
    fn splits_at_a_real_speaker_change() {
        let words = vec![
            w("Tom,", 0, 400), w("can", 400, 600), w("you", 600, 800), w("do", 800, 1000), w("that?", 1000, 1400),
            w("Yep,", 1600, 1900), w("I'll", 1900, 2100), w("take", 2100, 2400), w("it.", 2400, 2800),
        ];
        let runs = align_words(&words, &[turn(0, 1500, 0), turn(1500, 3000, 1)]);
        assert_eq!(runs, vec![(Some(0), 0..5), (Some(1), 5..9)]);
    }

    #[test]
    fn a_single_stray_word_does_not_split() {
        let words = vec![w("a", 0, 500), w("b", 500, 1000), w("c", 1000, 1200), w("d", 1200, 1700), w("e", 1700, 2200)];
        let turns = [turn(0, 1000, 0), turn(1000, 1200, 1), turn(1200, 3000, 0)];
        assert_eq!(align_words(&words, &turns), vec![(Some(0), 0..5)]);
    }

    #[test]
    fn words_outside_turns_attach_to_nearest() {
        let words = vec![w("a", 3000, 3500)];
        assert_eq!(align_words(&words, &[turn(0, 2800, 2)]), vec![(Some(2), 0..1)]);
        assert_eq!(align_words(&words, &[turn(0, 1000, 2)]), vec![(None, 0..1)]);
    }

    #[test]
    fn editing_operations() {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (mid, _, _) = store::create_meeting(&db, "m", root.path(), true, true, "keep_forever").unwrap();
        let a = insert_cluster(&db, &mid, "Speaker 1", SegmentSource::System, None, "unknown", None).unwrap();
        let b = insert_cluster(&db, &mid, "Speaker 2", SegmentSource::System, None, "unknown", None).unwrap();
        let words = vec![w("one", 0, 100), w("two", 100, 200), w("three", 200, 300)];
        let seg = insert_segment(&db, &NewSegment { meeting_id: &mid, source: SegmentSource::System, start_ms: 0, end_ms: 300, text: "one two three", words: &words, person_id: None, speaker_confidence: None }).unwrap();
        db.with(|c| c.execute("UPDATE transcript_segments SET speaker_cluster_id = ?2 WHERE id = ?1", params![seg, a])).unwrap();
        let tom = people::create(&db, &people::PersonInput { display_name: "Tom".into(), email: None, is_self: None }).unwrap();

        split_segment(&db, &seg, 2, Some(&b)).unwrap();
        let segs = db.with(|c| list_segments(c, &mid)).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "one two");
        assert_eq!(segs[1].text, "three");
        assert_eq!(segs[1].speaker_cluster_id.as_deref(), Some(b.as_str()));

        set_cluster_person(&db, &b, Some(&tom.id)).unwrap();
        let segs = db.with(|c| list_segments(c, &mid)).unwrap();
        assert_eq!(segs[1].person_id.as_deref(), Some(tom.id.as_str()));

        merge_clusters(&db, &a, &b).unwrap();
        let clusters = list_clusters(&db, &mid).unwrap();
        assert_eq!(clusters.len(), 1);
        assert!(clusters[0].confirmed);
        assert_eq!(clusters[0].segment_count, 2);
        let segs = db.with(|c| list_segments(c, &mid)).unwrap();
        assert!(segs.iter().all(|s| s.person_id.as_deref() == Some(tom.id.as_str())));

        edit_segment_text(&db, &segs[0].id, "One, two.").unwrap();
        let orig: String = db.with(|c| c.query_row("SELECT original_text FROM transcript_segments WHERE id = ?1", [&segs[0].id], |r| r.get(0))).unwrap();
        assert_eq!(orig, "one two three");
        let rev: i64 = db.with(|c| c.query_row("SELECT transcript_revision FROM meetings WHERE id = ?1", [&mid], |r| r.get(0))).unwrap();
        assert!(rev >= 3);
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::audio::capture::TrackInfo;
    use crate::meetings::processor::{Job, Processor, ProcessorDeps};
    use crate::meetings::MeetingStatus;
    use crate::models::manager::ModelManager;
    use crate::storage::secrets::MemorySecretStore;

    /// Transcribe + diarize a real two-speaker recording, with one speaker
    /// enrolled from a different part of the recording.
    /// MINUTES_DATA_DIR=... MINUTES_DIARIZATION_WAV=... cargo test --release two_speaker_pipeline -- --ignored --nocapture
    #[test]
    #[ignore]
    fn two_speaker_pipeline() {
        let data = std::path::PathBuf::from(std::env::var("MINUTES_DATA_DIR").unwrap());
        let wav = std::path::PathBuf::from(std::env::var("MINUTES_DIARIZATION_WAV").unwrap());
        let manager = Arc::new(ModelManager::new(Database::open(&data.join("minutes.db")).unwrap(), data.join("models")));
        let engine = Arc::new(SpeechEngine::new(manager));
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, _, dir) = store::create_meeting(&db, "Two", root.path(), true, true, "keep_forever").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("system-0.wav");
        std::fs::copy(&wav, &path).unwrap();
        let spec = hound::WavReader::open(&path).unwrap().spec();
        let info = TrackInfo { source: AudioSource::System, device_id: None, device_name: "Speakers".into(), sample_rate: spec.sample_rate, channels: spec.channels, path: path.clone(), start_offset_ms: 0 };
        let tid = store::insert_track(&db, &id, &info, 0).unwrap();
        store::update_track(&db, &tid, Some(1), Some("complete"), None).unwrap();
        store::finish_recording(&db, &id, 30_000, MeetingStatus::Processing).unwrap();

        // Enroll "Alice" from speaker 0's first turn (0.03–2.76 s) + (7.8–11.1 s).
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let cipher = VoiceprintCipher::load_or_create(secrets.as_ref()).unwrap();
        let audio = crate::audio::wav::read_16k_mono(&wav).unwrap();
        let emb = SherpaEmbedder::load(&engine.embedding_model().unwrap(), 2).unwrap();
        let alice = people::create(&db, &people::PersonInput { display_name: "Alice".into(), email: None, is_self: None }).unwrap();
        let prints = vec![emb.embed(&audio[480..44_000]).unwrap(), emb.embed(&audio[124_700..177_800]).unwrap()];
        people::add_embeddings(&db, &cipher, &alice.id, &prints, "enrollment", None, None).unwrap();

        let settings = Arc::new(RwLock::new(AppSettings::default()));
        let step = Arc::new(SpeakerRefinement { db: db.clone(), engine: engine.clone(), settings: settings.clone(), secrets });
        let processor = Processor::start(ProcessorDeps {
            db: db.clone(),
            engine,
            sink: Arc::new(|_, _| {}),
            settings,
            is_recording: Arc::new(|| false),
            speaker_step: Some(step),
            on_ready: None,
        });
        processor.enqueue(Job { meeting_id: id.clone(), live: None });
        for _ in 0..1200 {
            if store::get_summary(&db, &id).unwrap().unwrap().status != MeetingStatus::Processing {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let clusters = list_clusters(&db, &id).unwrap();
        let segs = db.with(|c| list_segments(c, &id)).unwrap();
        for c in &clusters {
            println!("{} {:?} segs={} ms={}", c.label, c.identity, c.segment_count, c.speaking_ms);
        }
        for s in &segs {
            let label = clusters.iter().find(|c| Some(&c.id) == s.speaker_cluster_id.as_ref()).map(|c| c.label.clone()).unwrap_or_default();
            println!("{:>6} {:<10} {}", s.start_ms, label, s.text);
        }
        assert_eq!(clusters.len(), 2);
        assert!(clusters.iter().any(|c| matches!(&c.identity, SpeakerIdentity::Known { person_id, .. } if *person_id == alice.id)));
        assert!(segs.iter().all(|s| s.speaker_cluster_id.is_some()));
    }
}
