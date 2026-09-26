//! First-run benchmark framework.
//!
//! Three stages, each optional so the app degrades gracefully:
//!
//! 1. **Synthetic** (no models needed, < 1 s): multi-threaded memory
//!    bandwidth and FMA throughput. Used to *estimate* speech and LLM speed
//!    before anything is downloaded.
//! 2. **ASR**: transcribe a bundled speech sample with the installed ASR
//!    model and measure load time and real-time factor.
//! 3. **LLM**: a short structured prompt against the installed LLM,
//!    measuring load time, prompt/generation throughput, stability and
//!    whether the structured response validates.
//!
//! Results are persisted with a hardware fingerprint so a hardware change
//! invalidates them.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::capabilities::HardwareSnapshot;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SyntheticBenchmark {
    /// Sustained multi-threaded read bandwidth, GB/s.
    pub memory_bandwidth_gbps: f64,
    /// Multi-threaded f32 FMA throughput, GFLOP/s.
    pub cpu_gflops: f64,
    pub threads: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AsrBenchmark {
    pub model_id: String,
    pub audio_seconds: f64,
    pub load_ms: u64,
    pub process_ms: u64,
    /// Real-time factor: processing time / audio duration. Lower is better.
    pub rtf: f64,
    pub peak_rss_bytes: Option<u64>,
    /// Transcript matched the reference closely enough to trust the timing.
    pub output_plausible: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LlmBenchmark {
    pub model_id: String,
    pub backend: String,
    pub context_tokens: u32,
    pub load_ms: u64,
    pub prompt_tokens: u32,
    pub prompt_tokens_per_second: f64,
    pub generated_tokens: u32,
    pub generation_tokens_per_second: f64,
    pub peak_rss_bytes: Option<u64>,
    /// Runtime stayed up and responded to every request.
    pub stable: bool,
    /// The structured response parsed and validated against the schema.
    pub structured_output_valid: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BenchmarkSummary {
    pub synthetic: Option<SyntheticBenchmark>,
    pub asr: Option<AsrBenchmark>,
    #[serde(default)]
    pub llm: Vec<LlmBenchmark>,
}

impl BenchmarkSummary {
    pub fn llm_for(&self, model_id: &str) -> Option<&LlmBenchmark> {
        self.llm.iter().filter(|b| b.model_id == model_id).last()
    }
}

/// Stable identity of the hardware a benchmark ran on. Memory is rounded to
/// whole GiB so the fingerprint is stable across small reporting changes.
pub fn hardware_fingerprint(hw: &HardwareSnapshot) -> String {
    let gpus: Vec<String> = hw.gpus.iter().filter_map(|g| g.model.clone()).collect();
    format!(
        "{:?}|{}|{}|{}|{}GiB|{}",
        hw.os(),
        hw.architecture,
        hw.cpu.model.as_deref().unwrap_or("?"),
        hw.cpu.logical_cores,
        (hw.memory.total_bytes as f64 / super::memory_budget::GIB as f64).round() as u64,
        gpus.join(",")
    )
}

/// Reference text of the bundled ASR sample (`test_wavs/en.wav`).
pub const ASR_SAMPLE_WORDS: &[&str] = &["ask", "not", "what", "your", "country", "can", "do", "for", "you"];

/// Transcription benchmark on the installed model. Returns `Err` only when
/// the model is not installed; runtime failures are reported in `error`.
pub fn run_asr(
    engine: &crate::speech::engine::SpeechEngine,
    models: &crate::models::manager::ModelManager,
) -> anyhow::Result<AsrBenchmark> {
    use crate::models::catalog::ASR_ID;
    let dir = models
        .installed_dir(ASR_ID)
        .ok_or_else(|| anyhow::anyhow!("Install the transcription model to run this test."))?;
    let audio = crate::audio::wav::read_16k_mono(&dir.join("test_wavs/en.wav"))?;
    let audio_seconds = audio.len() as f64 / 16_000.0;
    let mut out = AsrBenchmark { model_id: ASR_ID.into(), audio_seconds, ..Default::default() };
    engine.unload_asr(); // measure a cold load
    let threads = crate::meetings::processor::default_asr_threads();
    let t0 = Instant::now();
    let asr = match engine.asr(threads) {
        Ok(a) => a,
        Err(e) => {
            out.error = Some(e.to_string());
            return Ok(out);
        }
    };
    out.load_ms = t0.elapsed().as_millis() as u64;
    // Warm-up pass, then measure three passes and keep the best.
    let _ = asr.transcribe(&audio);
    let mut best = f64::INFINITY;
    let mut text = String::new();
    for _ in 0..3 {
        let t = Instant::now();
        match asr.transcribe(&audio) {
            Ok(r) => {
                best = best.min(t.elapsed().as_secs_f64());
                text = r.text;
            }
            Err(e) => {
                out.error = Some(e.to_string());
                return Ok(out);
            }
        }
    }
    out.process_ms = (best * 1000.0) as u64;
    out.rtf = best / audio_seconds.max(0.1);
    let lower = text.to_lowercase();
    let hits = ASR_SAMPLE_WORDS.iter().filter(|w| lower.contains(*w)).count();
    out.output_plausible = hits * 10 >= ASR_SAMPLE_WORDS.len() * 8;
    out.peak_rss_bytes = current_rss();
    Ok(out)
}

/// Resident memory of this process (approximate peak for the benchmark).
pub fn current_rss() -> Option<u64> {
    let pid = sysinfo::get_current_pid().ok()?;
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.memory())
}

/// Run the synthetic benchmark. CPU-bound and blocking: call from a blocking
/// task, never from the UI or async runtime threads.
pub fn run_synthetic(threads: usize) -> SyntheticBenchmark {
    let threads = threads.clamp(1, 32);
    let started = Instant::now();
    let bandwidth = measure_bandwidth(threads);
    let gflops = measure_gflops(threads);
    SyntheticBenchmark {
        memory_bandwidth_gbps: bandwidth,
        cpu_gflops: gflops,
        threads: threads as u32,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// Sum a 256 MiB buffer across threads several times; report the best pass.
fn measure_bandwidth(threads: usize) -> f64 {
    const BYTES: usize = 256 * 1024 * 1024;
    const PASSES: usize = 4;
    let words = BYTES / 8;
    let data: Vec<u64> = (0..words as u64).collect();
    let chunk = words.div_ceil(threads);
    let mut best = 0.0f64;
    for _ in 0..PASSES {
        let t = Instant::now();
        let total: u64 = std::thread::scope(|s| {
            let handles: Vec<_> = data
                .chunks(chunk)
                .map(|c| s.spawn(move || c.iter().fold(0u64, |a, &v| a.wrapping_add(v))))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or(0)).fold(0u64, u64::wrapping_add)
        });
        std::hint::black_box(total);
        let secs = t.elapsed().as_secs_f64().max(1e-9);
        best = best.max(BYTES as f64 / secs / 1e9);
    }
    best
}

/// Independent FMA chains per thread (vectorisable by the compiler).
fn measure_gflops(threads: usize) -> f64 {
    const LANES: usize = 64;
    const ITERS: usize = 2_000_000;
    let t = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut acc = [1.0f32; LANES];
                let m = std::hint::black_box(0.999_999f32);
                let a = std::hint::black_box(0.000_001f32);
                for _ in 0..ITERS {
                    for x in acc.iter_mut() {
                        *x = x.mul_add(m, a);
                    }
                }
                std::hint::black_box(acc);
            });
        }
    });
    let flops = (threads * ITERS * LANES * 2) as f64;
    flops / t.elapsed().as_secs_f64().max(1e-9) / 1e9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_benchmark_produces_positive_numbers() {
        let r = run_synthetic(2);
        assert!(r.memory_bandwidth_gbps > 0.1, "{r:?}");
        assert!(r.cpu_gflops > 0.1, "{r:?}");
    }

    #[test]
    fn fingerprint_ignores_available_memory() {
        let mut hw = crate::system::fixtures::load("mac-m1-16gb").hardware;
        let a = hardware_fingerprint(&hw);
        hw.memory.available_bytes /= 2;
        assert_eq!(a, hardware_fingerprint(&hw));
        hw.memory.total_bytes *= 2;
        assert_ne!(a, hardware_fingerprint(&hw));
    }

    #[test]
    #[ignore]
    fn asr_benchmark_on_real_model() {
        let data = std::path::PathBuf::from(std::env::var("MINUTES_DATA_DIR").unwrap());
        let db = crate::storage::Database::open(&data.join("minutes.db")).unwrap();
        let models = std::sync::Arc::new(crate::models::manager::ModelManager::new(db, data.join("models")));
        let engine = crate::speech::engine::SpeechEngine::new(models.clone());
        let r = run_asr(&engine, &models).unwrap();
        println!("{r:?}");
        assert!(r.error.is_none());
        assert!(r.output_plausible);
        assert!(r.rtf > 0.0 && r.rtf < 1.0);
    }

    #[test]
    fn latest_llm_benchmark_wins() {
        let s = BenchmarkSummary {
            llm: vec![
                LlmBenchmark { model_id: "m".into(), generation_tokens_per_second: 1.0, ..Default::default() },
                LlmBenchmark { model_id: "m".into(), generation_tokens_per_second: 2.0, ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(s.llm_for("m").unwrap().generation_tokens_per_second, 2.0);
    }
}
