//! Conversion of captured audio to the speech format: mono, 16 kHz, f32.
//!
//! Uses rubato's FFT resampler (band-limited, no aliasing) rather than naive
//! decimation. Input may arrive in arbitrary block sizes; output is produced
//! in whatever sizes the resampler yields.

use rubato::{FftFixedIn, Resampler};

pub const SPEECH_SAMPLE_RATE: u32 = 16_000;
const CHUNK_IN: usize = 1024;

/// Average interleaved channels into mono, appending to `out`.
pub fn downmix_into(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    out.extend(interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() * scale));
}

pub struct MonoResampler {
    inner: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    out_buf: Vec<Vec<f32>>,
}

impl MonoResampler {
    pub fn new(input_rate: u32) -> anyhow::Result<Self> {
        let inner = if input_rate == SPEECH_SAMPLE_RATE {
            None
        } else {
            Some(FftFixedIn::<f32>::new(input_rate as usize, SPEECH_SAMPLE_RATE as usize, CHUNK_IN, 2, 1)?)
        };
        let out_buf = inner.as_ref().map(|r| vec![vec![0.0; r.output_frames_max()]]).unwrap_or_default();
        Ok(MonoResampler { inner, pending: Vec::with_capacity(CHUNK_IN * 4), out_buf })
    }

    /// Feed mono samples; resampled output is appended to `out`.
    pub fn process(&mut self, mono: &[f32], out: &mut Vec<f32>) -> anyhow::Result<()> {
        let Some(r) = self.inner.as_mut() else {
            out.extend_from_slice(mono);
            return Ok(());
        };
        self.pending.extend_from_slice(mono);
        let mut offset = 0;
        while self.pending.len() - offset >= r.input_frames_next() {
            let need = r.input_frames_next();
            let input = [&self.pending[offset..offset + need]];
            let (_, produced) = r.process_into_buffer(&input, &mut self.out_buf, None)?;
            out.extend_from_slice(&self.out_buf[0][..produced]);
            offset += need;
        }
        self.pending.drain(..offset);
        Ok(())
    }

    /// Flush remaining samples (end of stream).
    pub fn finish(&mut self, out: &mut Vec<f32>) -> anyhow::Result<()> {
        let Some(r) = self.inner.as_mut() else { return Ok(()) };
        if !self.pending.is_empty() {
            let input = [&self.pending[..]];
            let (_, produced) = r.process_partial_into_buffer(Some(&input), &mut self.out_buf, None)?;
            out.extend_from_slice(&self.out_buf[0][..produced]);
            self.pending.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix_into(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn resamples_48k_to_16k_preserving_pitch() {
        for rate in [48_000u32, 44_100, 16_000, 96_000] {
            let input = sine(rate, 440.0, 2.0);
            let mut r = MonoResampler::new(rate).unwrap();
            let mut out = Vec::new();
            // Feed irregular block sizes like a real device.
            for block in input.chunks(333) {
                r.process(block, &mut out).unwrap();
            }
            r.finish(&mut out).unwrap();
            let expected = 32_000usize;
            assert!((out.len() as i64 - expected as i64).abs() < 1500, "{rate}: {} samples", out.len());
            // 440 Hz → ~880 crossings per second.
            let crossings = zero_crossings(&out[4000..20000]) as f32 / 1.0;
            assert!((crossings - 880.0).abs() < 20.0, "{rate}: {crossings} crossings");
        }
    }
}
