//! WAV persistence helpers, including header repair for crash recovery.
//!
//! Recordings are written with `hound`; the header is rewritten every few
//! seconds (`WavWriter::flush`), so after a crash at most the last few
//! seconds are missing from the header. `repair_header` fixes the sizes from
//! the actual file length so everything on disk is recovered.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Fix RIFF and data chunk sizes to match the file length. Returns the
/// number of audio bytes in the data chunk.
pub fn repair_header(path: &Path) -> std::io::Result<u64> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    let mut header = [0u8; 12];
    f.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a WAV file"));
    }
    // Walk chunks until "data".
    let mut pos: u64 = 12;
    loop {
        if pos + 8 > len {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no data chunk"));
        }
        f.seek(SeekFrom::Start(pos))?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch)?;
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"data" {
            let data_start = pos + 8;
            let mut data_len = len - data_start;
            data_len -= data_len % 4; // whole i16 stereo / f32 frames are ≤ 4-byte aligned
            f.seek(SeekFrom::Start(pos + 4))?;
            f.write_all(&(data_len.min(u32::MAX as u64) as u32).to_le_bytes())?;
            f.seek(SeekFrom::Start(4))?;
            f.write_all(&((data_start - 8 + data_len).min(u32::MAX as u64) as u32).to_le_bytes())?;
            f.set_len(data_start + data_len)?;
            f.sync_all()?;
            return Ok(data_len);
        }
        pos += 8 + size + (size & 1);
    }
}

/// Duration of a (possibly repaired) WAV file in milliseconds.
pub fn duration_ms(path: &Path) -> Option<u64> {
    let r = hound::WavReader::open(path).ok()?;
    let spec = r.spec();
    Some(r.duration() as u64 * 1000 / spec.sample_rate as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_truncated_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..48_000 {
            w.write_sample((i % 100) as i16).unwrap();
            w.write_sample(0i16).unwrap();
        }
        w.flush().unwrap(); // header now says 0.5 s ... then more samples are written
        for _ in 0..48_000 {
            w.write_sample(1i16).unwrap();
            w.write_sample(1i16).unwrap();
        }
        // Simulate a crash: drop without finalize by leaking the writer.
        std::mem::forget(w);
        // BufWriter contents were lost with forget; write raw bytes to model
        // data that reached disk after the last header update.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&vec![0u8; 48_000 * 4]).unwrap();
        drop(f);

        let bytes = repair_header(&path).unwrap();
        assert!(bytes >= 48_000 * 4 * 2 - 8192, "{bytes}");
        let ms = duration_ms(&path).unwrap();
        assert!(ms >= 1900, "{ms}");
    }
}

/// Streams a WAV file (any rate/channels, int or float) as mono 16 kHz
/// blocks, without loading the whole file. `start_ms`/`end_ms` select a
/// range; the callback receives each block and returns `false` to stop.
pub fn stream_16k_mono(
    path: &Path,
    start_ms: u64,
    end_ms: Option<u64>,
    mut on_block: impl FnMut(&[f32]) -> bool,
) -> anyhow::Result<()> {
    use super::resample::{downmix_into, MonoResampler};
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let rate = spec.sample_rate as u64;
    let total_frames = reader.duration() as u64;
    let start_frame = (start_ms * rate / 1000).min(total_frames);
    let end_frame = end_ms.map(|e| (e * rate / 1000).min(total_frames)).unwrap_or(total_frames);
    reader.seek(start_frame as u32)?;
    let mut resampler = MonoResampler::new(spec.sample_rate)?;
    let frames_per_block = (rate as usize / 2).max(1); // 0.5 s
    let mut remaining = end_frame.saturating_sub(start_frame) as usize;
    let mut interleaved: Vec<f32> = Vec::with_capacity(frames_per_block * channels);
    let mut mono = Vec::with_capacity(frames_per_block);
    let mut out = Vec::with_capacity(8_000);
    let int_scale = 1.0 / (1u64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
    while remaining > 0 {
        let n = remaining.min(frames_per_block);
        interleaved.clear();
        match spec.sample_format {
            hound::SampleFormat::Float => {
                for s in reader.samples::<f32>().take(n * channels) {
                    interleaved.push(s?);
                }
            }
            hound::SampleFormat::Int => {
                for s in reader.samples::<i32>().take(n * channels) {
                    interleaved.push(s? as f32 * int_scale);
                }
            }
        }
        if interleaved.is_empty() {
            break;
        }
        remaining -= interleaved.len() / channels;
        mono.clear();
        downmix_into(&interleaved[..interleaved.len() - interleaved.len() % channels], channels, &mut mono);
        out.clear();
        resampler.process(&mono, &mut out)?;
        if remaining == 0 {
            resampler.finish(&mut out)?;
        }
        if !out.is_empty() && !on_block(&out) {
            return Ok(());
        }
    }
    Ok(())
}

/// Convenience for short files (benchmarks, enrollment): whole file as mono 16 kHz.
pub fn read_16k_mono(path: &Path) -> anyhow::Result<Vec<f32>> {
    let mut all = Vec::new();
    stream_16k_mono(path, 0, None, |b| {
        all.extend_from_slice(b);
        true
    })?;
    Ok(all)
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn streams_range_as_16k() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..(48_000 * 3) {
            let v = ((i as f32 / 48_000.0 * 300.0 * std::f32::consts::TAU).sin() * 10_000.0) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let all = read_16k_mono(&path).unwrap();
        assert!((all.len() as i64 - 48_000).abs() < 1_200, "{}", all.len());
        let peak = all.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak - 10_000.0 / 32_768.0).abs() < 0.05, "{peak}");
        let mut n = 0;
        stream_16k_mono(&path, 1000, Some(2000), |b| {
            n += b.len();
            true
        })
        .unwrap();
        assert!((n as i64 - 16_000).abs() < 1_200, "{n}");
    }
}
