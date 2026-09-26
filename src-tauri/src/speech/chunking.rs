//! Splitting long speech spans into ASR-sized chunks.
//!
//! Spans are cut at the quietest short window inside a search range, never
//! with overlap, so no word is transcribed twice. Cutting in the lowest-energy
//! 30 ms window between the target and maximum length keeps the cut between
//! words in practice, so nothing is lost either.

use super::vad::SpeechSpan;
use super::SAMPLE_RATE;

const WINDOW: usize = (SAMPLE_RATE as usize * 30) / 1000;

#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    /// Preferred chunk length in seconds.
    pub target_s: f32,
    /// Hard maximum; the cut is chosen in `[target * 0.75, max]`.
    pub max_s: f32,
}

impl ChunkConfig {
    pub fn from_target(target_s: f32) -> Self {
        let target_s = target_s.clamp(5.0, 30.0);
        ChunkConfig { target_s, max_s: (target_s * 1.5).min(30.0).max(target_s) }
    }
}

/// Index of the quietest `WINDOW` in `samples[from..to]`.
fn quietest_point(samples: &[f32], from: usize, to: usize) -> usize {
    let mut best = to;
    let mut best_energy = f32::INFINITY;
    let mut i = from;
    while i + WINDOW <= to {
        let e: f32 = samples[i..i + WINDOW].iter().map(|s| s * s).sum();
        if e < best_energy {
            best_energy = e;
            best = i + WINDOW / 2;
        }
        i += WINDOW / 2;
    }
    best.min(samples.len())
}

pub fn split(span: SpeechSpan, cfg: ChunkConfig) -> Vec<SpeechSpan> {
    let max = (cfg.max_s * SAMPLE_RATE as f32) as usize;
    let min_cut = (cfg.target_s * 0.75 * SAMPLE_RATE as f32) as usize;
    if span.samples.len() <= max {
        return vec![span];
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    let samples = span.samples;
    while samples.len() - offset > max {
        let cut = quietest_point(&samples, offset + min_cut, offset + max);
        let cut = cut.clamp(offset + 1, offset + max);
        out.push(SpeechSpan { start_sample: span.start_sample + offset as u64, samples: samples[offset..cut].to_vec() });
        offset = cut;
    }
    out.push(SpeechSpan { start_sample: span.start_sample + offset as u64, samples: samples[offset..].to_vec() });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect()
    }

    #[test]
    fn short_spans_are_untouched() {
        let s = SpeechSpan { start_sample: 10, samples: tone(12.0) };
        let out = split(s, ChunkConfig::from_target(20.0));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn cuts_at_the_quiet_gap_without_losing_samples() {
        // 40 s of "speech" with a 100 ms pause at 18.0 s.
        let mut samples = tone(40.0);
        let gap = (18.0 * SAMPLE_RATE as f32) as usize;
        for s in &mut samples[gap..gap + 1600] {
            *s = 0.0;
        }
        let total = samples.len();
        let out = split(SpeechSpan { start_sample: 1000, samples }, ChunkConfig::from_target(20.0));
        assert!(out.len() >= 2);
        let first_end = out[0].samples.len();
        assert!((first_end as i64 - gap as i64 - 800).abs() < 800, "cut at {first_end}, gap at {gap}");
        // Contiguous, no overlap, nothing missing.
        let mut pos = 1000u64;
        for c in &out {
            assert_eq!(c.start_sample, pos);
            pos += c.samples.len() as u64;
        }
        assert_eq!(pos, 1000 + total as u64);
        assert!(out.iter().all(|c| c.samples.len() <= 30 * SAMPLE_RATE as usize));
    }
}
