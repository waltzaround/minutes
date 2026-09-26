//! Silero VAD via sherpa-onnx, with an absolute sample timeline.
//!
//! The detector keeps its own sample counter. When the input skips ahead
//! (live blocks dropped under load), we flush and restart it at the new
//! position so reported offsets always match the meeting timeline.

use std::path::Path;

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use super::SAMPLE_RATE;

const WINDOW: usize = 512;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub threshold: f32,
    pub min_silence_s: f32,
    pub min_speech_s: f32,
    /// Hard upper bound for one speech segment. Kept above the ASR chunk
    /// target so that our own low-energy splitter (see `chunking`) makes the
    /// cut instead of the VAD cutting mid-word.
    pub max_speech_s: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        VadConfig { threshold: 0.5, min_silence_s: 0.35, min_speech_s: 0.25, max_speech_s: 45.0 }
    }
}

#[derive(Debug, Clone)]
pub struct SpeechSpan {
    /// Absolute start in 16 kHz samples on the meeting timeline.
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

pub struct SpeechDetector {
    vad: VoiceActivityDetector,
    base: u64,
    fed: u64,
    pending: Vec<f32>,
}

impl SpeechDetector {
    pub fn new(model: &Path, cfg: &VadConfig) -> anyhow::Result<Self> {
        let c = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model.display().to_string()),
                threshold: cfg.threshold,
                min_silence_duration: cfg.min_silence_s,
                min_speech_duration: cfg.min_speech_s,
                window_size: WINDOW as i32,
                max_speech_duration: cfg.max_speech_s,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&c, cfg.max_speech_s + 15.0)
            .ok_or_else(|| anyhow::anyhow!("the speech detection model could not be loaded"))?;
        Ok(SpeechDetector { vad, base: 0, fed: 0, pending: Vec::with_capacity(WINDOW * 4) })
    }

    /// Position (absolute samples) the next input is expected at.
    pub fn expected_position(&self) -> u64 {
        self.base + self.fed + self.pending.len() as u64
    }

    /// Feed audio that starts at `start_sample`. Returns completed spans.
    pub fn accept(&mut self, start_sample: u64, samples: &[f32]) -> Vec<SpeechSpan> {
        let mut out = Vec::new();
        if start_sample != self.expected_position() {
            // Discontinuity: finish what we have and restart at the new position.
            out.extend(self.flush());
            self.vad.reset();
            self.base = start_sample;
            self.fed = 0;
            self.pending.clear();
        }
        self.pending.extend_from_slice(samples);
        let whole = self.pending.len() - self.pending.len() % WINDOW;
        for chunk in self.pending[..whole].chunks(WINDOW) {
            self.vad.accept_waveform(chunk);
        }
        self.fed += whole as u64;
        self.pending.drain(..whole);
        self.drain(&mut out);
        out
    }

    /// End of stream: emit any speech still in progress.
    pub fn flush(&mut self) -> Vec<SpeechSpan> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let mut tail = std::mem::take(&mut self.pending);
            let n = tail.len() as u64;
            tail.resize(WINDOW, 0.0);
            self.vad.accept_waveform(&tail);
            self.fed += n;
        }
        self.vad.flush();
        self.drain(&mut out);
        out
    }

    fn drain(&mut self, out: &mut Vec<SpeechSpan>) {
        while let Some(seg) = self.vad.front() {
            out.push(SpeechSpan { start_sample: self.base + seg.start().max(0) as u64, samples: seg.samples().to_vec() });
            self.vad.pop();
        }
    }
}
