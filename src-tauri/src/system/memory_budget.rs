//! Memory budgeting and model-fit calculation.
//!
//! Model eligibility is never decided from total RAM alone. Instead we build a
//! budget from total memory, currently available memory, memory pressure, GPU
//! memory (unified or discrete) and fixed reserves, and then ask whether a
//! specific model at a specific context size fits alongside (or after) the
//! speech models.
//!
//! All reserve constants here are deliberately conservative planning values.
//! They are not measurements; measured peak memory from benchmarks is recorded
//! separately and can tighten these numbers later.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;
pub const GIB: u64 = 1024 * MIB;

/// Memory kept free for the operating system: 20% of physical memory,
/// clamped to [2 GiB, 6 GiB].
pub fn os_reserve_bytes(total_bytes: u64) -> u64 {
    (total_bytes / 5).clamp(2 * GIB, 6 * GIB)
}

/// WebView, Rust runtime, audio buffers and SQLite.
pub const APP_OVERHEAD_BYTES: u64 = 600 * MIB;

/// Room left for the meeting client itself (Teams, Zoom, a browser tab with
/// Meet). Meetings are the primary use case, so this is always reserved.
pub const MEETING_APP_ALLOWANCE_BYTES: u64 = 1536 * MIB;

/// Memory we refuse to consume from what is *currently* available, so the
/// machine never gets pushed into heavy swap merely because a model fits.
pub const LIVE_FREE_FLOOR_BYTES: u64 = 1 * GIB;

/// Required slack on top of a model's estimated footprint.
pub const FIT_SAFETY_FACTOR: f64 = 1.10;

/// VRAM kept free on a discrete GPU for the desktop compositor and driver.
pub const VRAM_RESERVE_BYTES: u64 = 768 * MIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum MemoryPressure {
    Low,
    Medium,
    High,
}

impl MemoryPressure {
    /// Fraction of live available memory we are willing to plan against.
    fn live_factor(self) -> f64 {
        match self {
            MemoryPressure::Low => 1.0,
            MemoryPressure::Medium => 0.85,
            MemoryPressure::High => 0.6,
        }
    }
}

/// Inputs for the budget. Built from `SystemCapabilities` but kept separate so
/// the calculation is trivially testable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryBudgetInput {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub pressure: Option<MemoryPressure>,
    /// Apple Silicon style memory shared between CPU and GPU.
    pub unified_memory: bool,
    /// Metal `recommendedMaxWorkingSetSize` (Apple) – the practical limit for
    /// GPU-resident buffers in unified memory.
    pub gpu_working_set_bytes: Option<u64>,
    /// Largest discrete GPU that llama.cpp can actually offload to (CUDA or
    /// Vulkan available). `None` for CPU-only or unusable GPUs.
    pub discrete_vram_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryBudget {
    pub total_bytes: u64,
    pub os_reserve_bytes: u64,
    pub app_overhead_bytes: u64,
    pub meeting_app_allowance_bytes: u64,
    /// Budget for tier planning: what the machine can sustain when it is in a
    /// normal state (total minus all reserves).
    pub planning_bytes: u64,
    /// Budget for decisions made right now (loading a model), derived from
    /// currently available memory and pressure. Never larger than planning.
    pub live_bytes: u64,
    pub pressure: Option<MemoryPressure>,
    pub unified_memory: bool,
    /// Limit for GPU-resident buffers in unified memory, if known.
    pub gpu_working_set_bytes: Option<u64>,
    /// Usable discrete VRAM after the driver/compositor reserve.
    pub discrete_vram_usable_bytes: u64,
}

impl MemoryBudget {
    pub fn compute(input: &MemoryBudgetInput) -> Self {
        let os_reserve = os_reserve_bytes(input.total_bytes);
        let planning = input
            .total_bytes
            .saturating_sub(os_reserve)
            .saturating_sub(APP_OVERHEAD_BYTES)
            .saturating_sub(MEETING_APP_ALLOWANCE_BYTES);

        let factor = input.pressure.map(MemoryPressure::live_factor).unwrap_or(1.0);
        let live_raw = input.available_bytes.saturating_sub(LIVE_FREE_FLOOR_BYTES);
        let live = ((live_raw as f64) * factor) as u64;

        let discrete = input
            .discrete_vram_bytes
            .map(|v| v.saturating_sub(VRAM_RESERVE_BYTES))
            .unwrap_or(0);

        MemoryBudget {
            total_bytes: input.total_bytes,
            os_reserve_bytes: os_reserve,
            app_overhead_bytes: APP_OVERHEAD_BYTES,
            meeting_app_allowance_bytes: MEETING_APP_ALLOWANCE_BYTES,
            planning_bytes: planning,
            live_bytes: live.min(planning),
            pressure: input.pressure,
            unified_memory: input.unified_memory,
            gpu_working_set_bytes: input.gpu_working_set_bytes,
            discrete_vram_usable_bytes: discrete,
        }
    }
}

/// Estimated resident memory of a model once loaded.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MemoryFootprint {
    /// Weights resident in memory (approximately the file size for GGUF/ONNX).
    pub weights_bytes: u64,
    /// KV-cache bytes per context token (attention layers only).
    pub kv_bytes_per_token: u64,
    /// Context-independent state (e.g. Mamba SSM state) and runtime buffers.
    pub fixed_overhead_bytes: u64,
}

impl MemoryFootprint {
    pub fn total_at_context(&self, context_tokens: u32) -> u64 {
        self.weights_bytes
            + self.kv_bytes_per_token * context_tokens as u64
            + self.fixed_overhead_bytes
    }
}

/// Which budget the fit is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetBasis {
    /// Normal-state planning (capability tier, recommendations).
    Planning,
    /// Right now (about to load a model).
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FitMode {
    /// LLM can stay loaded alongside the speech models.
    Concurrent,
    /// LLM fits only after the speech models have been unloaded.
    Sequential,
    DoesNotFit,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelFit {
    pub mode: FitMode,
    pub context_tokens: u32,
    /// Footprint including the safety factor.
    pub required_bytes: u64,
    /// Portion placed in discrete VRAM (0 for CPU/unified).
    pub gpu_resident_bytes: u64,
    /// Fraction of layers to offload to a discrete GPU (1.0 on unified memory
    /// when the working set allows it).
    pub gpu_offload_fraction: f32,
    /// System memory left over after the model (and speech models when
    /// concurrent) are resident. Negative headroom is reported as 0 with
    /// `DoesNotFit`.
    pub headroom_bytes: u64,
}

impl ModelFit {
    pub fn fits(&self) -> bool {
        self.mode != FitMode::DoesNotFit
    }
}

fn with_safety(bytes: u64) -> u64 {
    (bytes as f64 * FIT_SAFETY_FACTOR).ceil() as u64
}

/// Check whether an LLM fits at `context_tokens`, given the speech working set
/// that is active during meetings.
pub fn assess_llm_fit(
    budget: &MemoryBudget,
    basis: BudgetBasis,
    llm: &MemoryFootprint,
    context_tokens: u32,
    speech_working_set_bytes: u64,
) -> ModelFit {
    let system_pool = match basis {
        BudgetBasis::Planning => budget.planning_bytes,
        BudgetBasis::Live => budget.live_bytes,
    };
    let required = with_safety(llm.total_at_context(context_tokens));
    let speech = with_safety(speech_working_set_bytes);

    // Split between discrete VRAM and system RAM. Only weights and KV cache
    // are offloadable; we treat the whole footprint proportionally, which is
    // how llama.cpp's per-layer offload behaves to a first approximation.
    let gpu_resident = required.min(budget.discrete_vram_usable_bytes);
    let system_required = required - gpu_resident;

    // On unified memory the GPU working-set limit caps what Metal can map.
    // Exceeding it forces partial CPU execution, not failure, so it only
    // affects the offload fraction.
    let offload_fraction = if budget.discrete_vram_usable_bytes > 0 {
        gpu_resident as f32 / required.max(1) as f32
    } else if budget.unified_memory {
        match budget.gpu_working_set_bytes {
            Some(ws) if ws > 0 => (ws as f32 / required.max(1) as f32).min(1.0),
            _ => 1.0,
        }
    } else {
        0.0
    };

    let mode = if system_required + speech <= system_pool {
        FitMode::Concurrent
    } else if system_required.max(speech) <= system_pool {
        FitMode::Sequential
    } else {
        FitMode::DoesNotFit
    };

    let used = match mode {
        FitMode::Concurrent => system_required + speech,
        FitMode::Sequential => system_required.max(speech),
        FitMode::DoesNotFit => system_pool,
    };

    ModelFit {
        mode,
        context_tokens,
        required_bytes: required,
        gpu_resident_bytes: gpu_resident,
        gpu_offload_fraction: offload_fraction,
        headroom_bytes: system_pool.saturating_sub(used),
    }
}

/// Whether the speech stack alone fits (the minimum for a usable app).
pub fn speech_fits(budget: &MemoryBudget, basis: BudgetBasis, speech_working_set_bytes: u64) -> bool {
    let pool = match basis {
        BudgetBasis::Planning => budget.planning_bytes,
        BudgetBasis::Live => budget.live_bytes,
    };
    with_safety(speech_working_set_bytes) <= pool
}

/// Largest context (from `candidates`, descending preference) at which the
/// LLM fits in the requested mode.
pub fn best_context(
    budget: &MemoryBudget,
    basis: BudgetBasis,
    llm: &MemoryFootprint,
    speech_working_set_bytes: u64,
    candidates: &[u32],
    require_concurrent: bool,
) -> Option<ModelFit> {
    candidates.iter().copied().find_map(|ctx| {
        let fit = assess_llm_fit(budget, basis, llm, ctx, speech_working_set_bytes);
        let ok = match fit.mode {
            FitMode::Concurrent => true,
            FitMode::Sequential => !require_concurrent,
            FitMode::DoesNotFit => false,
        };
        ok.then_some(fit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(total_gib: u64, avail_gib: u64) -> MemoryBudgetInput {
        MemoryBudgetInput {
            total_bytes: total_gib * GIB,
            available_bytes: avail_gib * GIB,
            ..Default::default()
        }
    }

    const SMALL: MemoryFootprint = MemoryFootprint {
        weights_bytes: 3 * GIB,
        kv_bytes_per_token: 64 * KIB,
        fixed_overhead_bytes: 300 * MIB,
    };

    #[test]
    fn os_reserve_is_clamped() {
        assert_eq!(os_reserve_bytes(8 * GIB), 2 * GIB);
        assert_eq!(os_reserve_bytes(16 * GIB), 16 * GIB / 5);
        assert_eq!(os_reserve_bytes(64 * GIB), 6 * GIB);
    }

    #[test]
    fn planning_budget_subtracts_all_reserves() {
        let b = MemoryBudget::compute(&input(16, 10));
        assert_eq!(
            b.planning_bytes,
            16 * GIB - 16 * GIB / 5 - APP_OVERHEAD_BYTES - MEETING_APP_ALLOWANCE_BYTES
        );
        assert!(b.live_bytes <= b.planning_bytes);
    }

    #[test]
    fn live_budget_shrinks_under_pressure() {
        let mut i = input(32, 12);
        let low = MemoryBudget::compute(&i).live_bytes;
        i.pressure = Some(MemoryPressure::High);
        let high = MemoryBudget::compute(&i).live_bytes;
        assert!(high < low);
        assert_eq!(high, ((11 * GIB) as f64 * 0.6) as u64);
    }

    #[test]
    fn live_budget_never_exceeds_planning() {
        let b = MemoryBudget::compute(&input(8, 8));
        assert_eq!(b.live_bytes, b.planning_bytes);
    }

    #[test]
    fn tiny_machine_cannot_fit_model() {
        let b = MemoryBudget::compute(&input(4, 2));
        let fit = assess_llm_fit(&b, BudgetBasis::Planning, &SMALL, 4096, GIB);
        assert_eq!(fit.mode, FitMode::DoesNotFit);
    }

    #[test]
    fn eight_gib_is_sequential_for_small_model() {
        let b = MemoryBudget::compute(&input(8, 4));
        let fit = assess_llm_fit(&b, BudgetBasis::Planning, &SMALL, 4096, 1200 * MIB);
        assert_eq!(fit.mode, FitMode::Sequential);
    }

    #[test]
    fn sixteen_gib_is_concurrent_for_small_model() {
        let b = MemoryBudget::compute(&input(16, 10));
        let fit = assess_llm_fit(&b, BudgetBasis::Planning, &SMALL, 16384, 1200 * MIB);
        assert_eq!(fit.mode, FitMode::Concurrent);
        assert!(fit.headroom_bytes > 0);
    }

    #[test]
    fn discrete_vram_takes_load_off_system_memory() {
        let big = MemoryFootprint {
            weights_bytes: 18 * GIB,
            kv_bytes_per_token: 32 * KIB,
            fixed_overhead_bytes: 500 * MIB,
        };
        let cpu_only = MemoryBudget::compute(&input(16, 12));
        assert!(!assess_llm_fit(&cpu_only, BudgetBasis::Planning, &big, 8192, GIB).fits());

        let mut with_gpu = input(16, 12);
        with_gpu.discrete_vram_bytes = Some(12 * GIB);
        let b = MemoryBudget::compute(&with_gpu);
        let fit = assess_llm_fit(&b, BudgetBasis::Planning, &big, 8192, GIB);
        assert!(fit.fits());
        assert!(fit.gpu_offload_fraction > 0.5 && fit.gpu_offload_fraction < 1.0);
    }

    #[test]
    fn best_context_prefers_largest_that_fits() {
        let b = MemoryBudget::compute(&input(8, 5));
        let fit = best_context(&b, BudgetBasis::Planning, &SMALL, 1200 * MIB, &[32768, 16384, 8192, 4096], false)
            .expect("some context fits");
        assert!(fit.context_tokens < 32768);
    }

    #[test]
    fn kv_cache_is_part_of_the_requirement() {
        let at_4k = SMALL.total_at_context(4096);
        let at_32k = SMALL.total_at_context(32768);
        assert_eq!(at_32k - at_4k, 64 * KIB * (32768 - 4096));
    }
}
