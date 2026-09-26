//! Speaker diarization with NVIDIA Nemotron 3 Diarization (streaming
//! Sortformer, up to 8 speakers), run by the bundled NeMo-Speech.cpp runtime
//! (`nemo-speech`, C++/ggml, Metal on Apple Silicon, Vulkan/CPU on Windows).
//!
//! The runtime is a managed subprocess, like `llama-server`: audio is handed
//! over as a temporary 16 kHz mono WAV and segments come back as JSON. Unlike
//! clustering-based pipelines there is no clustering threshold to tune: the
//! model assigns speakers in order of first appearance and handles overlap.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::storage::settings::InferenceBackendPreference;

#[derive(Debug, Clone, PartialEq)]
pub struct DiarizedTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Speaker index within this run, 0-based, in order of first appearance.
    pub speaker: u32,
}

pub trait Diarizer: Send + Sync {
    fn diarize(&self, samples_16k: &[f32]) -> anyhow::Result<Vec<DiarizedTurn>>;
}

pub fn runtime_binary(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(if cfg!(windows) { "nemo-speech.exe" } else { "nemo-speech" })
}

pub struct NemoDiarizer {
    binary: PathBuf,
    model: PathBuf,
    device: &'static str,
    work_dir: PathBuf,
}

#[derive(Deserialize)]
struct Output {
    segments: Vec<Segment>,
}

#[derive(Deserialize)]
struct Segment {
    start: f64,
    end: f64,
    speaker: u32,
}

/// Parse `nemo-speech diarize --format json` output (1-based speakers).
pub fn parse_output(json: &str) -> anyhow::Result<Vec<DiarizedTurn>> {
    let out: Output = serde_json::from_str(json.trim())?;
    let mut turns: Vec<DiarizedTurn> = out
        .segments
        .into_iter()
        .filter(|s| s.end > s.start && s.speaker >= 1)
        .map(|s| DiarizedTurn {
            start_ms: (s.start.max(0.0) * 1000.0).round() as u64,
            end_ms: (s.end * 1000.0).round() as u64,
            speaker: s.speaker - 1,
        })
        .collect();
    turns.sort_by_key(|t| (t.start_ms, t.end_ms));
    Ok(turns)
}

impl NemoDiarizer {
    pub fn new(runtime_dir: &Path, model: &Path, backend: InferenceBackendPreference, work_dir: &Path) -> anyhow::Result<Self> {
        let binary = runtime_binary(runtime_dir);
        anyhow::ensure!(binary.exists(), "the speaker separation runtime is not installed with this app build");
        anyhow::ensure!(model.exists(), "the speaker separation model is not installed");
        let device = match backend {
            InferenceBackendPreference::Cpu => "cpu",
            _ => "auto",
        };
        Ok(NemoDiarizer { binary, model: model.to_path_buf(), device, work_dir: work_dir.to_path_buf() })
    }

    fn write_wav(&self, samples: &[f32]) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(&self.work_dir)?;
        let path = self.work_dir.join(format!("diarize-{}.wav", uuid::Uuid::new_v4()));
        let spec = hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec)?;
        for &s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
        }
        w.finalize()?;
        Ok(path)
    }
}

impl Diarizer for NemoDiarizer {
    fn diarize(&self, samples: &[f32]) -> anyhow::Result<Vec<DiarizedTurn>> {
        if samples.len() < 16_000 {
            return Ok(Vec::new());
        }
        let wav = self.write_wav(samples)?;
        let out_json = wav.with_extension("json");
        let log = wav.with_extension("log");
        let result = (|| -> anyhow::Result<Vec<DiarizedTurn>> {
            let mut cmd = Command::new(&self.binary);
            cmd.current_dir(self.binary.parent().unwrap_or(Path::new(".")))
                .arg("diarize")
                .arg(&wav)
                .arg("--model")
                .arg(&self.model)
                .args(["--format", "json", "--device", self.device, "--force", "--output"])
                .arg(&out_json)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(std::fs::File::create(&log)?));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let mut child = cmd.spawn()?;
            // Generous: first run on Metal compiles shaders (~20 s); then it
            // runs many times faster than real time.
            let audio_secs = samples.len() as u64 / 16_000;
            let timeout = Duration::from_secs(180 + audio_secs);
            let started = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("speaker separation took too long");
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            if !status.success() {
                let tail = std::fs::read_to_string(&log).unwrap_or_default();
                tracing::error!(log = tail.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>(), "nemo-speech diarize failed");
                anyhow::bail!("speaker separation failed");
            }
            tracing::info!(ms = started.elapsed().as_millis() as u64, audio_secs, "diarization finished");
            parse_output(&std::fs::read_to_string(&out_json)?)
        })();
        for p in [&wav, &out_json, &log] {
            let _ = std::fs::remove_file(p);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_json_to_zero_based_turns() {
        let json = r#"{
          "file": "x.wav",
          "segments": [
            {"start": 2.531, "end": 7.809, "speaker": 2},
            {"start": 0.000, "end": 2.779, "speaker": 1},
            {"start": 3.0, "end": 3.0, "speaker": 1}
          ]
        }"#;
        let t = parse_output(json).unwrap();
        assert_eq!(t.len(), 2, "zero-length segments dropped");
        assert_eq!(t[0], DiarizedTurn { start_ms: 0, end_ms: 2779, speaker: 0 });
        assert_eq!(t[1], DiarizedTurn { start_ms: 2531, end_ms: 7809, speaker: 1 });
    }

    #[test]
    fn missing_runtime_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        assert!(NemoDiarizer::new(dir.path(), &dir.path().join("m.gguf"), InferenceBackendPreference::Auto, dir.path()).is_err());
    }
}
