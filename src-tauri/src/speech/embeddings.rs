//! Speaker embeddings (voiceprints) via sherpa-onnx.

use std::path::Path;

use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

use super::SAMPLE_RATE;

/// Minimum audio for a usable embedding.
pub const MIN_EMBEDDING_SAMPLES: usize = SAMPLE_RATE as usize * 3 / 2;

pub trait EmbeddingModel: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, samples_16k: &[f32]) -> anyhow::Result<Vec<f32>>;
}

pub struct SherpaEmbedder {
    inner: SpeakerEmbeddingExtractor,
}

impl SherpaEmbedder {
    pub fn load(model: &Path, threads: u32) -> anyhow::Result<Self> {
        let inner = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(model.display().to_string()),
            num_threads: threads.max(1) as i32,
            ..Default::default()
        })
        .ok_or_else(|| anyhow::anyhow!("the voice recognition model could not be loaded"))?;
        Ok(SherpaEmbedder { inner })
    }
}

impl EmbeddingModel for SherpaEmbedder {
    fn dim(&self) -> usize {
        self.inner.dim().max(0) as usize
    }

    fn embed(&self, samples: &[f32]) -> anyhow::Result<Vec<f32>> {
        anyhow::ensure!(samples.len() >= MIN_EMBEDDING_SAMPLES, "not enough speech for a voiceprint");
        let stream = self.inner.create_stream().ok_or_else(|| anyhow::anyhow!("embedding stream"))?;
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        stream.input_finished();
        anyhow::ensure!(self.inner.is_ready(&stream), "not enough speech for a voiceprint");
        let v = self.inner.compute(&stream).ok_or_else(|| anyhow::anyhow!("embedding failed"))?;
        Ok(normalize(v))
    }
}

pub fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut d, mut x, mut y) = (0.0f32, 0.0f32, 0.0f32);
    for (p, q) in a.iter().zip(b) {
        d += p * q;
        x += p * p;
        y += q * q;
    }
    if x > 0.0 && y > 0.0 {
        d / (x.sqrt() * y.sqrt())
    } else {
        0.0
    }
}

pub fn mean(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = vectors.first()?;
    let dim = first.len();
    let mut acc = vec![0.0f32; dim];
    for v in vectors.iter().filter(|v| v.len() == dim) {
        for (a, x) in acc.iter_mut().zip(v) {
            *a += x;
        }
    }
    Some(normalize(acc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        let m = mean(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        assert!((m[0] - m[1]).abs() < 1e-6);
        assert!((m.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
    }
}
