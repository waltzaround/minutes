//! Voice enrollment: record 30–60 s of natural speech, cut it into several
//! speech windows, embed each, check they agree, and store them encrypted.
//! This is not model training; it only stores voiceprints.
//!
//! The enrollment recording is deleted afterwards unless the user explicitly
//! asks to keep it.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use crate::audio::capture::{AudioSource, CaptureEvent, CaptureHandle, CaptureRequest};
use crate::meetings::recorder::{meter_level, AudioLevel, EventSink};
use crate::speech::embeddings::{cosine, mean, EmbeddingModel};
use crate::speech::vad::{SpeechDetector, VadConfig};
use crate::speech::SAMPLE_RATE;

pub const MIN_SPEECH_SECONDS: f32 = 20.0;
const WINDOW_SECONDS: f32 = 6.0;
const MIN_WINDOW_SECONDS: f32 = 3.0;
const MIN_WINDOWS: usize = 3;
/// Mean pairwise similarity below this suggests noise or several voices.
/// Starting point from calibration (same speaker 0.51–0.74, different
/// speakers 0.09–0.36 for TitaNet-small); see docs/models.md.
const MIN_CONSISTENCY: f32 = 0.45;
/// Windows less similar than this to the profile centroid are dropped.
const MIN_TO_CENTROID: f32 = 0.6;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnrollmentResult {
    pub accepted: bool,
    pub speech_seconds: f32,
    pub samples: u32,
    pub consistency: Option<f32>,
    /// Plain-language explanation when not accepted.
    pub message: Option<String>,
    pub recording_kept_at: Option<String>,
}

struct Session {
    person_id: String,
    path: PathBuf,
    handle: CaptureHandle,
    pump: JoinHandle<()>,
}

#[derive(Default)]
pub struct EnrollmentManager {
    active: Mutex<Option<Session>>,
}

impl EnrollmentManager {
    pub fn start(&self, person_id: &str, mic_device: Option<String>, dir: &std::path::Path, sink: EventSink) -> anyhow::Result<()> {
        let mut guard = self.active.lock();
        if let Some(old) = guard.take() {
            let _ = old.handle.stop();
            let _ = std::fs::remove_file(&old.path);
        }
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("enrollment-{}.wav", uuid::Uuid::new_v4()));
        let (tx, rx) = mpsc::channel::<CaptureEvent>();
        let handle = CaptureHandle::start(
            CaptureRequest { source: AudioSource::Microphone, device_id: mic_device, path: path.clone(), start_offset_ms: 0 },
            tx,
            None,
        )?;
        let pump = std::thread::spawn(move || {
            while let Ok(ev) = rx.recv() {
                if let CaptureEvent::Level { source, rms, .. } = ev {
                    let l = AudioLevel { source, level: meter_level(rms), active: rms > 0.005 };
                    sink(crate::events::AUDIO_LEVEL, serde_json::to_value(&l).unwrap_or_default());
                }
            }
        });
        *guard = Some(Session { person_id: person_id.to_string(), path, handle, pump });
        Ok(())
    }

    pub fn cancel(&self) {
        if let Some(s) = self.active.lock().take() {
            let _ = s.handle.stop();
            let _ = s.pump.join();
            let _ = std::fs::remove_file(&s.path);
        }
    }

    /// Stop recording and return (person id, recording path).
    pub fn stop(&self) -> anyhow::Result<(String, PathBuf)> {
        let s = self.active.lock().take().ok_or_else(|| anyhow::anyhow!("No voice recording is in progress."))?;
        let result = s.handle.stop();
        let _ = s.pump.join();
        if let Some(e) = result.error {
            anyhow::bail!("The recording failed: {e}");
        }
        Ok((s.person_id, s.path))
    }
}

/// Cut speech spans into windows of about `WINDOW_SECONDS`.
pub fn speech_windows(spans: Vec<Vec<f32>>) -> (Vec<Vec<f32>>, f32) {
    let window = (WINDOW_SECONDS * SAMPLE_RATE as f32) as usize;
    let min = (MIN_WINDOW_SECONDS * SAMPLE_RATE as f32) as usize;
    let total: usize = spans.iter().map(Vec::len).sum();
    let mut windows = Vec::new();
    let mut current = Vec::with_capacity(window);
    for span in spans {
        for chunk in span.chunks(window) {
            current.extend_from_slice(chunk);
            if current.len() >= window {
                windows.push(std::mem::replace(&mut current, Vec::with_capacity(window)));
            }
        }
    }
    if current.len() >= min {
        windows.push(current);
    }
    (windows, total as f32 / SAMPLE_RATE as f32)
}

/// Embed windows, drop outliers and check consistency.
pub fn build_profile(windows: &[Vec<f32>], embedder: &dyn EmbeddingModel) -> Result<(Vec<Vec<f32>>, f32), String> {
    let embeddings: Vec<Vec<f32>> = windows.iter().filter_map(|w| embedder.embed(w).ok()).collect();
    if embeddings.len() < MIN_WINDOWS {
        return Err("Not enough clear speech was captured. Please speak naturally for about 30–60 seconds.".into());
    }
    let centroid = mean(&embeddings).expect("non-empty");
    let kept: Vec<Vec<f32>> = embeddings.into_iter().filter(|e| cosine(e, &centroid) >= MIN_TO_CENTROID).collect();
    if kept.len() < MIN_WINDOWS {
        return Err("The recording sounded inconsistent. Try again somewhere quieter, with only you speaking.".into());
    }
    let mut sum = 0.0;
    let mut n = 0;
    for i in 0..kept.len() {
        for j in i + 1..kept.len() {
            sum += cosine(&kept[i], &kept[j]);
            n += 1;
        }
    }
    let consistency = if n > 0 { sum / n as f32 } else { 0.0 };
    if consistency < MIN_CONSISTENCY {
        return Err("The recording sounded inconsistent. Try again somewhere quieter, with only you speaking.".into());
    }
    Ok((kept, consistency))
}

/// Analyse an enrollment recording: VAD → windows → embeddings.
pub fn analyse_recording(
    path: &std::path::Path,
    vad_model: &std::path::Path,
    embedder: &dyn EmbeddingModel,
) -> anyhow::Result<(Result<(Vec<Vec<f32>>, f32), String>, f32)> {
    let audio = crate::audio::wav::read_16k_mono(path)?;
    let mut vad = SpeechDetector::new(vad_model, &VadConfig::default())?;
    let mut spans: Vec<Vec<f32>> = vad.accept(0, &audio).into_iter().map(|s| s.samples).collect();
    spans.extend(vad.flush().into_iter().map(|s| s.samples));
    let (windows, speech_seconds) = speech_windows(spans);
    if speech_seconds < MIN_SPEECH_SECONDS {
        return Ok((
            Err(format!(
                "Only {speech_seconds:.0} seconds of speech were detected. Please speak for about 30–60 seconds."
            )),
            speech_seconds,
        ));
    }
    Ok((build_profile(&windows, embedder), speech_seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeEmbedder(Box<dyn Fn(&[f32]) -> Vec<f32> + Send + Sync>);
    impl EmbeddingModel for FakeEmbedder {
        fn dim(&self) -> usize {
            2
        }
        fn embed(&self, s: &[f32]) -> anyhow::Result<Vec<f32>> {
            Ok((self.0)(s))
        }
    }

    #[test]
    fn windows_cover_speech() {
        let spans = vec![vec![0.1; 16_000 * 10], vec![0.1; 16_000 * 4], vec![0.1; 16_000]];
        let (w, secs) = speech_windows(spans);
        assert_eq!(secs, 15.0);
        assert_eq!(w.len(), 2, "6 s + 6 s, remainder 3 s kept");
        assert!(w.iter().all(|x| x.len() >= 16_000 * 3));
    }

    #[test]
    fn consistent_voice_is_accepted_and_mixed_voices_rejected() {
        let windows: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32; 100]).collect();
        let same = FakeEmbedder(Box::new(|s| vec![1.0, 0.05 * s[0]]));
        let (kept, c) = build_profile(&windows, &same).unwrap();
        assert_eq!(kept.len(), 5);
        assert!(c > 0.9);
        let mixed = FakeEmbedder(Box::new(|s| {
            let mut v = vec![0.0; 3];
            v[(s[0] as usize) % 3] = 1.0;
            v
        }));
        assert!(build_profile(&windows, &mixed).is_err());
    }
}
