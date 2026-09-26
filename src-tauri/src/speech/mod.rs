//! Local speech processing (sherpa-onnx): VAD, ASR, diarization and speaker
//! embeddings. Implementations sit behind small traits so they can be
//! replaced or mocked.

pub mod asr;
pub mod chunking;
pub mod diarization;
pub mod embeddings;
pub mod engine;
pub mod speaker_registry;
pub mod vad;

#[cfg(test)]
mod vad_tests;
#[cfg(test)]
mod diarization_calibration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const SAMPLE_RATE: u32 = 16_000;

pub fn samples_to_ms(samples: u64) -> u64 {
    samples * 1000 / SAMPLE_RATE as u64
}

pub fn ms_to_samples(ms: u64) -> u64 {
    ms * SAMPLE_RATE as u64 / 1000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Word {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcription {
    pub text: String,
    /// Word timings relative to the start of the transcribed audio.
    pub words: Vec<Word>,
}

/// Speech-to-text over one chunk of mono 16 kHz audio.
pub trait SpeechRecognizer: Send + Sync {
    fn transcribe(&self, samples: &[f32]) -> anyhow::Result<Transcription>;
}
