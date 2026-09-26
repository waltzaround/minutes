//! Real-time capture of one logical stream (microphone or system audio).
//!
//! ```text
//! device callback ──► lock-free ring buffer ──► worker thread
//!  (no locks, no I/O,                            ├─► WAV file (original rate/channels, 16-bit)
//!   no allocation)                               ├─► level meter events
//!                                                └─► mono 16 kHz blocks → speech pipeline
//! ```
//!
//! The audio callback never blocks: if the ring buffer is full (the worker
//! stalled), samples are dropped and counted, and a warning is raised. The
//! speech pipeline is fed with `try_send`, so slow transcription can never
//! back-pressure capture; the WAV on disk remains the source of truth and
//! the post-meeting pass reprocesses anything the live path skipped.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::devices;
use super::resample::{downmix_into, MonoResampler, SPEECH_SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AudioSource {
    Microphone,
    System,
}

impl AudioSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AudioSource::Microphone => "microphone",
            AudioSource::System => "system",
        }
    }
}

/// Mono 16 kHz audio for the speech pipeline. `start_sample` counts 16 kHz
/// samples from the start of the meeting (not the stream), so both sources
/// share one timeline.
#[derive(Debug, Clone)]
pub struct SpeechBlock {
    pub source: AudioSource,
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone)]
pub enum CaptureEvent {
    Level { source: AudioSource, rms: f32, peak: f32, any_signal: bool },
    /// The device went away or the stream failed. Capture of this source has
    /// stopped; the other source is unaffected.
    Failed { source: AudioSource, message: String, disconnected: bool },
    Overrun { source: AudioSource, dropped_frames: u64 },
    Progress { source: AudioSource, frames_written: u64 },
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub source: AudioSource,
    pub device_id: Option<String>,
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub path: PathBuf,
    /// Offset of this segment from the meeting start.
    pub start_offset_ms: u64,
}

pub struct CaptureRequest {
    pub source: AudioSource,
    /// Persisted cpal device id; `None` = default device.
    pub device_id: Option<String>,
    pub path: PathBuf,
    pub start_offset_ms: u64,
}

pub struct CaptureHandle {
    pub info: TrackInfo,
    stream: Option<cpal::Stream>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<CaptureResult>>,
}

#[derive(Debug, Clone)]
pub struct CaptureResult {
    pub frames_written: u64,
    pub dropped_frames: u64,
    pub error: Option<String>,
}

/// Ring buffer capacity: 4 seconds of audio at the device rate.
const RING_SECONDS: usize = 4;
const WORKER_TICK: Duration = Duration::from_millis(15);
const HEADER_FLUSH_INTERVAL: Duration = Duration::from_secs(2);
const LEVEL_INTERVAL: Duration = Duration::from_millis(80);
/// ~0.5 s speech blocks keep the channel small and latency low.
const SPEECH_BLOCK_SAMPLES: usize = SPEECH_SAMPLE_RATE as usize / 2;

fn stream_error_is_disconnect(e: &cpal::Error) -> bool {
    matches!(
        e.kind(),
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated | cpal::ErrorKind::DeviceChanged
    )
}

impl CaptureHandle {
    /// Open the device and start capturing. Returns once the stream is
    /// playing; failures are returned so the caller can warn the user rather
    /// than silently record only one stream.
    pub fn start(
        req: CaptureRequest,
        events: Sender<CaptureEvent>,
        speech: Option<SyncSender<SpeechBlock>>,
    ) -> anyhow::Result<Self> {
        let input = req.source == AudioSource::Microphone;
        let device = devices::resolve_device(req.device_id.as_deref(), input).ok_or_else(|| {
            anyhow::anyhow!(if input { "No microphone is available." } else { "No output device is available to capture meeting audio from." })
        })?;
        let device_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| device.to_string());
        let device_id = device.id().ok().map(|i| i.to_string());
        // Loopback uses the output device's own format.
        let supported = if input { device.default_input_config()? } else { device.default_output_config()? };
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let channels = config.channels.max(1);
        let sample_rate = config.sample_rate;

        let capacity = sample_rate as usize * channels as usize * RING_SECONDS;
        let (producer, consumer) = HeapRb::<f32>::new(capacity).split();
        let dropped = Arc::new(AtomicU64::new(0));

        let err_events = events.clone();
        let source = req.source;
        let on_error = move |e: cpal::Error| {
            let disconnected = stream_error_is_disconnect(&e);
            tracing::warn!(source = source.as_str(), error = %e, "audio stream error");
            if disconnected || !matches!(e.kind(), cpal::ErrorKind::Xrun) {
                let _ = err_events.send(CaptureEvent::Failed { source, message: e.to_string(), disconnected });
            }
        };

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::F64 => build::<f64>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::I16 => build::<i16>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::I32 => build::<i32>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::I24 => build::<cpal::I24>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::U16 => build::<u16>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::U8 => build::<u8>(&device, &config, producer, dropped.clone(), on_error)?,
            SampleFormat::I8 => build::<i8>(&device, &config, producer, dropped.clone(), on_error)?,
            other => anyhow::bail!("unsupported audio sample format {other:?}"),
        };

        if let Some(parent) = req.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(&req.path, spec)?;
        let resampler = MonoResampler::new(sample_rate)?;

        let info = TrackInfo {
            source: req.source,
            device_id,
            device_name,
            sample_rate,
            channels,
            path: req.path.clone(),
            start_offset_ms: req.start_offset_ms,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let stop = stop.clone();
            let ctx = WorkerCtx {
                source: req.source,
                channels: channels as usize,
                start_sample_16k: req.start_offset_ms * SPEECH_SAMPLE_RATE as u64 / 1000,
                consumer,
                writer,
                resampler,
                events,
                speech,
                dropped,
            };
            std::thread::Builder::new()
                .name(format!("capture-{}", req.source.as_str()))
                .spawn(move || ctx.run(stop))?
        };

        stream.play()?;
        tracing::info!(source = req.source.as_str(), device = info.device_name, sample_rate, channels, ?format, "capture started");
        Ok(CaptureHandle { info, stream: Some(stream), stop, worker: Some(worker) })
    }

    /// Stop the stream, drain buffered audio and finalise the WAV file.
    pub fn stop(mut self) -> CaptureResult {
        self.finish()
    }

    fn finish(&mut self) -> CaptureResult {
        if let Some(s) = self.stream.take() {
            let _ = s.pause();
            drop(s);
        }
        self.stop.store(true, Ordering::Relaxed);
        self.worker
            .take()
            .and_then(|w| w.join().ok())
            .unwrap_or(CaptureResult { frames_written: 0, dropped_frames: 0, error: Some("capture worker panicked".into()) })
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.finish();
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut producer: ringbuf::HeapProd<f32>,
    dropped: Arc<AtomicU64>,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as u64;
    // Pre-allocated conversion buffer; grows only if the device delivers an
    // unusually large block (then stays large).
    let mut scratch: Vec<f32> = Vec::with_capacity(8192);
    let stream = device.build_input_stream::<T, _, _>(
        *config,
        move |data: &[T], _info: &cpal::InputCallbackInfo| {
            scratch.clear();
            scratch.extend(data.iter().map(|s| f32::from_sample_(*s)));
            let pushed = producer.push_slice(&scratch);
            if pushed < scratch.len() {
                dropped.fetch_add((scratch.len() - pushed) as u64 / channels, Ordering::Relaxed);
            }
        },
        on_error,
        None,
    )?;
    Ok(stream)
}

struct WorkerCtx {
    source: AudioSource,
    channels: usize,
    start_sample_16k: u64,
    consumer: ringbuf::HeapCons<f32>,
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    resampler: MonoResampler,
    events: Sender<CaptureEvent>,
    speech: Option<SyncSender<SpeechBlock>>,
    dropped: Arc<AtomicU64>,
}

impl WorkerCtx {
    fn run(mut self, stop: Arc<AtomicBool>) -> CaptureResult {
        let mut buf = vec![0.0f32; 16_384 - 16_384 % self.channels.max(1)];
        let mut mono = Vec::with_capacity(8192);
        let mut resampled = Vec::with_capacity(8192);
        let mut speech_pending: Vec<f32> = Vec::with_capacity(SPEECH_BLOCK_SAMPLES * 2);
        let mut speech_cursor = self.start_sample_16k;
        let mut frames_written: u64 = 0;
        let mut last_flush = Instant::now();
        let mut last_level = Instant::now();
        let mut level_sum = 0.0f64;
        let mut level_n = 0usize;
        let mut level_peak = 0.0f32;
        let mut any_signal = false;
        let mut reported_dropped = 0u64;
        let mut error: Option<String> = None;
        let mut speech_skipped = false;
        let mut carry: Vec<f32> = Vec::with_capacity(buf.len() + 16);

        loop {
            let stopping = stop.load(Ordering::Relaxed);
            let mut got_any = false;
            loop {
                let n = self.consumer.pop_slice(&mut buf);
                if n == 0 {
                    break;
                }
                got_any = true;
                // Process whole frames only; a partial frame waits in `carry`.
                carry.extend_from_slice(&buf[..n]);
                let whole = carry.len() - carry.len() % self.channels;
                if whole == 0 {
                    continue;
                }
                let frames: Vec<f32> = carry.drain(..whole).collect();
                if let Err(e) = self.consume(&frames, &mut mono, &mut resampled, &mut speech_pending, &mut speech_cursor, &mut speech_skipped) {
                    error.get_or_insert(e.to_string());
                }
                frames_written += (frames.len() / self.channels) as u64;
                accumulate_level(&frames, &mut level_sum, &mut level_n, &mut level_peak, &mut any_signal);
            }

            if last_level.elapsed() >= LEVEL_INTERVAL {
                last_level = Instant::now();
                let rms = if level_n > 0 { (level_sum / level_n as f64).sqrt() as f32 } else { 0.0 };
                let _ = self.events.send(CaptureEvent::Level { source: self.source, rms, peak: level_peak, any_signal });
                level_sum = 0.0;
                level_n = 0;
                level_peak = 0.0;
                any_signal = false;
                let d = self.dropped.load(Ordering::Relaxed);
                if d > reported_dropped {
                    let _ = self.events.send(CaptureEvent::Overrun { source: self.source, dropped_frames: d - reported_dropped });
                    reported_dropped = d;
                }
            }
            if last_flush.elapsed() >= HEADER_FLUSH_INTERVAL {
                last_flush = Instant::now();
                if let Err(e) = self.writer.flush() {
                    error.get_or_insert(format!("could not write audio file: {e}"));
                }
                let _ = self.events.send(CaptureEvent::Progress { source: self.source, frames_written });
            }
            if stopping && !got_any {
                break;
            }
            if !got_any {
                std::thread::sleep(WORKER_TICK);
            }
        }

        // Flush the speech tail and finalise the file.
        let mut tail = Vec::new();
        if self.resampler.finish(&mut tail).is_ok() {
            speech_pending.extend_from_slice(&tail);
        }
        if !speech_pending.is_empty() {
            self.send_speech(std::mem::take(&mut speech_pending), &mut speech_cursor, &mut speech_skipped);
        }
        if let Err(e) = self.writer.finalize() {
            error.get_or_insert(format!("could not finish audio file: {e}"));
        }
        let _ = self.events.send(CaptureEvent::Progress { source: self.source, frames_written });
        CaptureResult { frames_written, dropped_frames: self.dropped.load(Ordering::Relaxed), error }
    }

    fn consume(
        &mut self,
        interleaved: &[f32],
        mono: &mut Vec<f32>,
        resampled: &mut Vec<f32>,
        speech_pending: &mut Vec<f32>,
        speech_cursor: &mut u64,
        speech_skipped: &mut bool,
    ) -> anyhow::Result<()> {
        for &s in interleaved {
            self.writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        if self.speech.is_none() {
            return Ok(());
        }
        mono.clear();
        downmix_into(interleaved, self.channels, mono);
        resampled.clear();
        self.resampler.process(mono, resampled)?;
        speech_pending.extend_from_slice(resampled);
        while speech_pending.len() >= SPEECH_BLOCK_SAMPLES {
            let block: Vec<f32> = speech_pending.drain(..SPEECH_BLOCK_SAMPLES).collect();
            self.send_speech(block, speech_cursor, speech_skipped);
        }
        Ok(())
    }

    fn send_speech(&mut self, samples: Vec<f32>, cursor: &mut u64, skipped: &mut bool) {
        let len = samples.len() as u64;
        if let Some(tx) = &self.speech {
            match tx.try_send(SpeechBlock { source: self.source, start_sample: *cursor, samples }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    if !*skipped {
                        tracing::warn!(source = self.source.as_str(), "live transcription is behind; audio is still being recorded");
                        *skipped = true;
                    }
                }
                Err(TrySendError::Disconnected(_)) => self.speech = None,
            }
        }
        *cursor += len;
    }
}

fn accumulate_level(samples: &[f32], sum: &mut f64, n: &mut usize, peak: &mut f32, any: &mut bool) {
    for &s in samples {
        *sum += (s as f64) * (s as f64);
        let a = s.abs();
        if a > *peak {
            *peak = a;
        }
        if s != 0.0 {
            *any = true;
        }
    }
    *n += samples.len();
}

/// Receive side helper used by tests and the recorder.
pub type CaptureEvents = Receiver<CaptureEvent>;
