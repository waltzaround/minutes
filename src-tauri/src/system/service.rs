//! Orchestrates detection, benchmarks and assessment, with caching and
//! persistence.

use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::benchmark::{self, AsrBenchmark, BenchmarkSummary, LlmBenchmark, SyntheticBenchmark};
use super::capabilities::{self, HardwareSnapshot};
use super::fixtures;
use super::profile::{self, CapabilityAssessment};
use crate::storage::{new_id, now, Database};

/// Everything the UI needs about this machine.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemCapabilities {
    pub hardware: HardwareSnapshot,
    pub assessment: CapabilityAssessment,
    pub benchmarks: BenchmarkSummary,
    pub detected_at: String,
    /// Set when a development fixture is being simulated instead of this
    /// machine. The UI shows a prominent banner in that case.
    pub simulated_fixture: Option<String>,
}

pub struct SystemService {
    data_dir: PathBuf,
    db: Database,
    cached: RwLock<Option<(HardwareSnapshot, String)>>,
}

impl SystemService {
    pub fn new(data_dir: PathBuf, db: Database) -> Self {
        SystemService { data_dir, db, cached: RwLock::new(None) }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Full detection (first launch, or on request). Blocking.
    pub fn detect_full(&self) -> HardwareSnapshot {
        let mut hw = capabilities::detect_baseline(&self.data_dir);
        hw.audio = crate::audio::devices::detect_audio(&hw);
        *self.cached.write() = Some((hw.clone(), now()));
        hw
    }

    /// Lightweight check: reuse static facts, refresh memory/disk/permissions.
    pub fn detect_light(&self) -> HardwareSnapshot {
        let cached = self.cached.read().clone();
        match cached {
            Some((mut hw, _)) => {
                capabilities::refresh_dynamic(&mut hw, &self.data_dir);
                hw.audio = crate::audio::devices::detect_audio(&hw);
                *self.cached.write() = Some((hw.clone(), now()));
                hw
            }
            None => self.detect_full(),
        }
    }

    pub fn capabilities(&self, refresh_full: bool, simulate: Option<&str>) -> SystemCapabilities {
        if let Some(id) = simulate.filter(|_| cfg!(debug_assertions)) {
            if let Some(f) = fixtures::all().into_iter().find(|f| f.id == id) {
                let bench = f.benchmark.clone().unwrap_or_default();
                return SystemCapabilities {
                    assessment: profile::assess(&f.hardware, Some(&bench)),
                    hardware: f.hardware,
                    benchmarks: bench,
                    detected_at: now(),
                    simulated_fixture: Some(f.id),
                };
            }
        }
        let hw = if refresh_full { self.detect_full() } else { self.detect_light() };
        let bench = self.load_benchmarks(&hw);
        SystemCapabilities {
            assessment: profile::assess(&hw, Some(&bench)),
            hardware: hw,
            benchmarks: bench,
            detected_at: now(),
            simulated_fixture: None,
        }
    }

    /// Latest result per stage/model for the current hardware fingerprint.
    pub fn load_benchmarks(&self, hw: &HardwareSnapshot) -> BenchmarkSummary {
        let fp = benchmark::hardware_fingerprint(hw);
        let rows: Vec<(String, String)> = self
            .db
            .with(|c| {
                let mut stmt = c.prepare(
                    "SELECT kind, result_json FROM benchmark_results
                     WHERE hardware_fingerprint = ?1 ORDER BY created_at ASC",
                )?;
                let rows = stmt.query_map([fp], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect()
            })
            .unwrap_or_default();
        let mut summary = BenchmarkSummary::default();
        for (kind, json) in rows {
            match kind.as_str() {
                "synthetic" => summary.synthetic = serde_json::from_str::<SyntheticBenchmark>(&json).ok(),
                "asr" => summary.asr = serde_json::from_str::<AsrBenchmark>(&json).ok(),
                "llm" => {
                    if let Ok(b) = serde_json::from_str::<LlmBenchmark>(&json) {
                        summary.llm.retain(|x| x.model_id != b.model_id);
                        summary.llm.push(b);
                    }
                }
                _ => {}
            }
        }
        summary
    }

    pub fn save_benchmark<T: Serialize>(
        &self,
        hw: &HardwareSnapshot,
        kind: &str,
        model_id: Option<&str>,
        passed: bool,
        result: &T,
    ) -> rusqlite::Result<()> {
        let json = serde_json::to_string(result).expect("benchmark serializes");
        let fp = benchmark::hardware_fingerprint(hw);
        self.db.with(|c| {
            c.execute(
                "INSERT INTO benchmark_results (id, kind, model_id, hardware_fingerprint, passed, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![new_id(), kind, model_id, fp, passed as i32, json, now()],
            )
            .map(|_| ())
        })
    }

    pub fn cached_hardware(&self) -> Option<HardwareSnapshot> {
        self.cached.read().as_ref().map(|(hw, _)| hw.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmarks_are_scoped_to_hardware() {
        let db = Database::open_in_memory().unwrap();
        let svc = SystemService::new(std::env::temp_dir(), db);
        let hw = fixtures::load("mac-m1-16gb").hardware;
        let syn = SyntheticBenchmark { memory_bandwidth_gbps: 60.0, cpu_gflops: 100.0, threads: 8, duration_ms: 500 };
        svc.save_benchmark(&hw, "synthetic", None, true, &syn).unwrap();
        let llm = LlmBenchmark { model_id: "a".into(), generation_tokens_per_second: 5.0, ..Default::default() };
        svc.save_benchmark(&hw, "llm", Some("a"), true, &llm).unwrap();
        let llm2 = LlmBenchmark { model_id: "a".into(), generation_tokens_per_second: 7.0, ..Default::default() };
        svc.save_benchmark(&hw, "llm", Some("a"), true, &llm2).unwrap();

        let loaded = svc.load_benchmarks(&hw);
        assert_eq!(loaded.synthetic, Some(syn));
        assert_eq!(loaded.llm.len(), 1);
        assert_eq!(loaded.llm[0].generation_tokens_per_second, 7.0);

        let other = fixtures::load("mac-m4max-48gb").hardware;
        assert!(svc.load_benchmarks(&other).synthetic.is_none());
    }

    #[test]
    fn simulation_uses_fixture() {
        let svc = SystemService::new(std::env::temp_dir(), Database::open_in_memory().unwrap());
        let caps = svc.capabilities(false, Some("win-intel-8gb"));
        assert_eq!(caps.simulated_fixture.as_deref(), Some("win-intel-8gb"));
        assert_eq!(caps.hardware.memory.total_bytes, 8 * crate::system::memory_budget::GIB);
    }
}

/// Writes `SystemCapabilities` for every hardware fixture to
/// `src/dev/generated/` for the browser-only development mock
/// (`vite` + `?mock`). Run with `MINUTES_EXPORT_SAMPLES=1 cargo test export_capability_samples`.
#[cfg(test)]
mod samples {
    use super::*;

    #[test]
    fn export_capability_samples() {
        if std::env::var_os("MINUTES_EXPORT_SAMPLES").is_none() {
            return;
        }
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/dev/generated");
        std::fs::create_dir_all(&out).unwrap();
        let svc = SystemService::new(std::env::temp_dir(), Database::open_in_memory().unwrap());
        let mut all = serde_json::Map::new();
        for f in fixtures::all() {
            let caps = svc.capabilities(false, Some(&f.id));
            all.insert(f.id.clone(), serde_json::to_value(caps).unwrap());
        }
        let manifests = serde_json::to_value(crate::models::catalog::manifests()).unwrap();
        let doc = serde_json::json!({ "capabilities": all, "manifests": manifests });
        std::fs::write(out.join("samples.json"), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }
}
