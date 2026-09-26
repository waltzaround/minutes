//! Parakeet TDT 0.6B v3 via sherpa-onnx (offline transducer, CPU).
//!
//! Parakeet is offline-only in sherpa-onnx, so "live" transcription is VAD
//! segmentation + offline decoding of each bounded chunk (the approach used
//! by upstream's own streaming example).

use std::path::Path;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

use super::{SpeechRecognizer, Transcription, Word, SAMPLE_RATE};

pub struct ParakeetRecognizer {
    inner: OfflineRecognizer,
}

impl ParakeetRecognizer {
    /// Load from an installed model directory. Takes a second or two.
    pub fn load(dir: &Path, threads: u32) -> anyhow::Result<Self> {
        let file = |name: &str| -> anyhow::Result<String> {
            let p = dir.join(name);
            anyhow::ensure!(p.exists(), "transcription model file {name} is missing");
            Ok(p.display().to_string())
        };
        let mut c = OfflineRecognizerConfig::default();
        c.model_config.transducer = OfflineTransducerModelConfig {
            encoder: Some(file("encoder.int8.onnx")?),
            decoder: Some(file("decoder.int8.onnx")?),
            joiner: Some(file("joiner.int8.onnx")?),
        };
        c.model_config.tokens = Some(file("tokens.txt")?);
        c.model_config.model_type = Some("nemo_transducer".into());
        c.model_config.num_threads = threads.max(1) as i32;
        c.model_config.provider = Some("cpu".into());
        c.decoding_method = Some("greedy_search".into());
        let inner = OfflineRecognizer::create(&c).ok_or_else(|| anyhow::anyhow!("the transcription model could not be loaded"))?;
        Ok(ParakeetRecognizer { inner })
    }
}

impl SpeechRecognizer for ParakeetRecognizer {
    fn transcribe(&self, samples: &[f32]) -> anyhow::Result<Transcription> {
        if samples.is_empty() {
            return Ok(Transcription::default());
        }
        let stream = self.inner.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.inner.decode(&stream);
        let r = stream.get_result().ok_or_else(|| anyhow::anyhow!("no transcription result"))?;
        let audio_ms = (samples.len() as u64 * 1000) / SAMPLE_RATE as u64;
        let words = group_words(&r.tokens, r.timestamps.as_deref(), r.durations.as_deref(), audio_ms);
        Ok(Transcription { text: r.text.trim().to_string(), words })
    }
}

/// Group sub-word tokens into words. A token beginning with whitespace (or
/// SentencePiece's "▁") starts a new word. Word end is the next token's start
/// (or the token duration for the final token).
pub fn group_words(tokens: &[String], timestamps: Option<&[f32]>, durations: Option<&[f32]>, audio_ms: u64) -> Vec<Word> {
    let Some(ts) = timestamps else {
        return Vec::new();
    };
    let mut words: Vec<Word> = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        let start = (ts.get(i).copied().unwrap_or(0.0).max(0.0) * 1000.0) as u64;
        let end = match (durations.and_then(|d| d.get(i)), ts.get(i + 1)) {
            (Some(d), _) if *d > 0.0 => start + (d * 1000.0) as u64,
            (_, Some(next)) => (next * 1000.0) as u64,
            _ => audio_ms.max(start),
        };
        let starts_word = tok.starts_with(' ') || tok.starts_with('▁') || words.is_empty();
        let clean = tok.replace('▁', " ");
        let is_punct = clean.trim().chars().all(|c| c.is_ascii_punctuation()) && !clean.trim().is_empty();
        if starts_word && !(is_punct && !words.is_empty()) {
            let text = clean.trim().to_string();
            if text.is_empty() {
                continue;
            }
            words.push(Word { text, start_ms: start, end_ms: end.max(start) });
        } else if let Some(w) = words.last_mut() {
            w.text.push_str(clean.trim());
            w.end_ms = end.max(w.end_ms);
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn groups_subword_tokens_into_words() {
        let tokens = toks(&[" Hel", "lo", " world", "."]);
        let ts = [0.0, 0.2, 0.5, 0.9];
        let words = group_words(&tokens, Some(&ts), None, 1200);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0], Word { text: "Hello".into(), start_ms: 0, end_ms: 500 });
        assert_eq!(words[1].text, "world.");
        assert_eq!(words[1].start_ms, 500);
        assert_eq!(words[1].end_ms, 1200);
    }

    #[test]
    fn handles_sentencepiece_marker_and_durations() {
        let tokens = toks(&["▁I", "'ll", "▁take", "▁it"]);
        let ts = [0.1, 0.2, 0.4, 0.7];
        let dur = [0.1, 0.1, 0.2, 0.2];
        let words = group_words(&tokens, Some(&ts), Some(&dur), 1000);
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["I'll", "take", "it"]);
        assert_eq!(words[2].end_ms, 900);
    }

    #[test]
    fn no_timestamps_means_no_words() {
        assert!(group_words(&toks(&[" a"]), None, None, 100).is_empty());
    }

    /// Transcribes the bundled benchmark sample with the installed model.
    /// `MINUTES_MODELS_DIR=... cargo test parakeet_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn parakeet_real_model() {
        let dir = std::path::PathBuf::from(std::env::var("MINUTES_MODELS_DIR").unwrap()).join(crate::models::catalog::ASR_ID);
        let t0 = std::time::Instant::now();
        let asr = ParakeetRecognizer::load(&dir, 4).unwrap();
        let load = t0.elapsed();
        let samples = crate::audio::wav::read_16k_mono(&dir.join("test_wavs/en.wav")).unwrap();
        let t1 = std::time::Instant::now();
        let out = asr.transcribe(&samples).unwrap();
        println!("load {load:?} decode {:?} for {} s\n{}\n{:?}", t1.elapsed(), samples.len() as f32 / 16000.0, out.text, out.words.iter().take(8).collect::<Vec<_>>());
        assert!(!out.text.is_empty());
        assert!(!out.words.is_empty());
    }
}
