//! Capability tiers (Basic / Standard / Enhanced / Full).
//!
//! The recommended tier is the minimum of three independent limits:
//!
//! 1. **Memory ceiling** — what fits in the memory budget (RAM, available
//!    memory, unified/discrete GPU memory, model size, KV cache, speech
//!    working set, OS margin). Never RAM alone.
//! 2. **Acceleration cap** — without a GPU path llama.cpp can use, the Full
//!    tier requires benchmark evidence before it is recommended.
//! 3. **Performance evidence** — benchmark results can promote a machine
//!    above its acceleration cap (e.g. a fast CPU-only desktop) or demote it
//!    (e.g. a 32 GB machine with very slow inference).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::benchmark::BenchmarkSummary;
use super::capabilities::HardwareSnapshot;
use super::memory_budget::{
    assess_llm_fit, best_context, speech_fits, BudgetBasis, FitMode, MemoryBudget, MemoryBudgetInput, ModelFit, GIB,
};
use super::requirements::{self, RequirementReport, SupportLevel};
use crate::models::catalog::{self, LlmSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CapabilityProfile {
    Basic,
    Standard,
    Enhanced,
    Full,
}

impl CapabilityProfile {
    pub fn label(self) -> &'static str {
        match self {
            CapabilityProfile::Basic => "Basic",
            CapabilityProfile::Standard => "Standard",
            CapabilityProfile::Enhanced => "Enhanced",
            CapabilityProfile::Full => "Full",
        }
    }
}

/// User-facing quality rating. Deliberately not a numeric score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Rating {
    Unavailable,
    Limited,
    Standard,
    Good,
    Excellent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AssessmentBasis {
    /// Hardware detection only.
    HardwareOnly,
    /// Hardware plus synthetic CPU/memory benchmark.
    Estimated,
    /// At least one real model benchmark (ASR or LLM).
    Benchmarked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Headline {
    /// "Your computer is ready"
    Ready,
    /// "Your computer is compatible"
    Compatible,
    /// "Limited performance"
    Limited,
    /// "Not supported" — speech itself is not viable.
    NotSupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProfileAdjustment {
    pub from: CapabilityProfile,
    pub to: CapabilityProfile,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProfileSettings {
    pub llm_model_id: Option<String>,
    pub context_tokens: u32,
    pub llm_fit: Option<ModelFit>,
    /// Speech models must be unloaded before the LLM loads.
    pub sequential_loading: bool,
    pub keep_llm_resident: bool,
    pub realtime_transcript: bool,
    pub diarization: bool,
    pub diarization_threads: u32,
    pub asr_threads: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InferenceSupport {
    pub asr_supported: bool,
    pub diarization_supported: bool,
    pub large_llm_supported: bool,
    pub small_llm_supported: bool,
    pub recommended_profile: CapabilityProfile,
    /// "metal", "cuda", "vulkan" or "cpu".
    pub llm_backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Ratings {
    pub transcription: Rating,
    pub speaker_recognition: Rating,
    pub meeting_summaries: Rating,
    /// True when ratings come from estimates rather than model benchmarks.
    pub estimated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CapabilityAssessment {
    pub requirements: RequirementReport,
    pub budget: MemoryBudget,
    pub memory_ceiling: Option<CapabilityProfile>,
    pub acceleration_cap: CapabilityProfile,
    pub profile: CapabilityProfile,
    pub basis: AssessmentBasis,
    pub adjustments: Vec<ProfileAdjustment>,
    pub settings: ProfileSettings,
    pub inference: InferenceSupport,
    pub ratings: Ratings,
    pub headline: Headline,
    /// False only when a benchmark shows speech processing cannot work.
    pub can_continue: bool,
    /// Plain-language notes for the result screen.
    pub notes: Vec<String>,
    /// Catalog model ids this profile needs.
    pub required_models: Vec<String>,
}

/// Contexts tried from most to least preferred.
const FULL_CONTEXTS: &[u32] = &[32768, 16384];
const ENHANCED_CONTEXTS: &[u32] = &[32768];
const STANDARD_CONTEXTS: &[u32] = &[16384];
const BASIC_CONTEXTS: &[u32] = &[8192, 4096];

/// Headroom beyond the concurrent footprint for the Enhanced tier (lets the
/// machine multitask alongside Teams/Meet/Zoom without swapping).
const ENHANCED_EXTRA_HEADROOM: u64 = 8 * GIB;

// Benchmark thresholds. These define the tiers, not universal truths; they
// are documented in docs/capability-detection.md.
const ASR_RTF_REALTIME_COMFORTABLE: f64 = 0.35;
const ASR_RTF_REALTIME_TIGHT: f64 = 0.6;
const ASR_RTF_MAX_VIABLE: f64 = 4.0;
const LLM_TPS_FULL: f64 = 20.0;
const LLM_TPS_ENHANCED: f64 = 10.0;
const LLM_TPS_STANDARD: f64 = 5.0;
const LLM_TPS_BASIC_MIN: f64 = 1.5;
const LARGE_LLM_MIN_GEN_TPS: f64 = 10.0;
const LARGE_LLM_MIN_PROMPT_TPS: f64 = 80.0;

pub fn budget_input(hw: &HardwareSnapshot) -> MemoryBudgetInput {
    MemoryBudgetInput {
        total_bytes: hw.memory.total_bytes,
        available_bytes: hw.memory.available_bytes,
        pressure: hw.memory.pressure,
        unified_memory: hw.gpus.iter().any(|g| g.unified_memory),
        gpu_working_set_bytes: hw.unified_gpu_working_set(),
        discrete_vram_bytes: hw.usable_discrete_vram(),
    }
}

pub fn llm_backend(hw: &HardwareSnapshot) -> &'static str {
    if hw.is_apple_silicon() && hw.gpus.iter().any(|g| g.metal) {
        "metal"
    } else if hw.gpus.iter().any(|g| g.cuda && !g.integrated) {
        "cuda"
    } else if hw.gpus.iter().any(|g| g.vulkan && !g.integrated) {
        "vulkan"
    } else {
        "cpu"
    }
}

fn memory_ceiling(budget: &MemoryBudget, meets_minimum: bool) -> (Option<CapabilityProfile>, bool) {
    let speech_full = catalog::speech_working_set_bytes(true);
    let speech_min = catalog::speech_working_set_bytes(false);
    let diarization = speech_fits(budget, BudgetBasis::Planning, speech_full);
    if !diarization && !speech_fits(budget, BudgetBasis::Planning, speech_min) {
        return (None, false);
    }
    let speech = if diarization { speech_full } else { speech_min };
    let large = catalog::large_llm();
    let small = catalog::small_llm();

    if best_context(budget, BudgetBasis::Planning, &large.footprint, speech, FULL_CONTEXTS, false).is_some() {
        return (Some(CapabilityProfile::Full), diarization);
    }
    if let Some(fit) = best_context(budget, BudgetBasis::Planning, &small.footprint, speech, ENHANCED_CONTEXTS, true) {
        if fit.headroom_bytes >= ENHANCED_EXTRA_HEADROOM {
            return (Some(CapabilityProfile::Enhanced), diarization);
        }
    }
    if meets_minimum
        && best_context(budget, BudgetBasis::Planning, &small.footprint, speech, STANDARD_CONTEXTS, true).is_some()
    {
        return (Some(CapabilityProfile::Standard), diarization);
    }
    (Some(CapabilityProfile::Basic), diarization)
}

fn acceleration_cap(hw: &HardwareSnapshot) -> (CapabilityProfile, Option<String>) {
    if hw.architecture == "x86_64" && hw.cpu.avx2 == Some(false) {
        return (CapabilityProfile::Basic, Some("This processor lacks AVX2, which local AI relies on.".into()));
    }
    if hw.cpu.physical_cores.is_some_and(|c| c < requirements::MIN_PHYSICAL_CORES) {
        return (CapabilityProfile::Standard, Some("Fewer than 4 processor cores.".into()));
    }
    match llm_backend(hw) {
        "metal" => (CapabilityProfile::Full, None),
        "cuda" | "vulkan" if hw.usable_discrete_vram().is_some_and(|v| v >= 6 * GIB) => {
            (CapabilityProfile::Full, None)
        }
        _ => (
            CapabilityProfile::Enhanced,
            Some("No GPU acceleration was found, so the largest model needs a benchmark first.".into()),
        ),
    }
}

fn tier_for_llm_tps(tps: f64) -> CapabilityProfile {
    if tps >= LLM_TPS_FULL {
        CapabilityProfile::Full
    } else if tps >= LLM_TPS_ENHANCED {
        CapabilityProfile::Enhanced
    } else if tps >= LLM_TPS_STANDARD {
        CapabilityProfile::Standard
    } else {
        CapabilityProfile::Basic
    }
}

/// Estimate generation speed of `target` from a measured benchmark of
/// `measured`. Token generation is dominated by reading the *active*
/// weights, so speed scales roughly with active parameter count. A 0.75
/// factor accounts for MoE routing overhead and is intentionally pessimistic.
pub fn estimate_generation_tps(measured: &LlmSpec, measured_tps: f64, target: &LlmSpec) -> f64 {
    let ratio = measured.active_params_billions as f64 / target.active_params_billions as f64;
    measured_tps * ratio * if target.mixture_of_experts { 0.75 } else { 1.0 }
}

/// Estimate generation speed from memory bandwidth alone (CPU inference is
/// bandwidth-bound; ~50% of peak bandwidth is typically achieved).
pub fn estimate_tps_from_bandwidth(bandwidth_gbps: f64, llm: &LlmSpec) -> f64 {
    let bytes_per_token = llm.active_weight_bytes() as f64;
    bandwidth_gbps * 1e9 * 0.5 / bytes_per_token.max(1.0)
}

pub fn assess(hw: &HardwareSnapshot, bench: Option<&BenchmarkSummary>) -> CapabilityAssessment {
    let requirements = requirements::evaluate(hw);
    let budget = MemoryBudget::compute(&budget_input(hw));
    let (ceiling, diarization_fits) = memory_ceiling(&budget, requirements.meets_official_minimum);
    let (accel_cap, accel_note) = acceleration_cap(hw);
    let small = catalog::small_llm();
    let large = catalog::large_llm();

    let mut notes = Vec::new();
    let mut adjustments = Vec::new();
    let mut speech_viable = ceiling.is_some();
    let mut realtime = true;

    let bench = bench.cloned().unwrap_or_default();
    let has_model_bench = bench.asr.is_some() || !bench.llm.is_empty();
    let basis = if has_model_bench {
        AssessmentBasis::Benchmarked
    } else if bench.synthetic.is_some() {
        AssessmentBasis::Estimated
    } else {
        AssessmentBasis::HardwareOnly
    };

    // --- Performance evidence -------------------------------------------
    let mut evidence_cap = accel_cap;
    let mut small_llm_ok = true;
    let mut large_llm_ok = true;

    if let Some(syn) = &bench.synthetic {
        // Synthetic results can only demote: they are estimates.
        let est_small = estimate_tps_from_bandwidth(syn.memory_bandwidth_gbps, small);
        let cap = tier_for_llm_tps(est_small).max(CapabilityProfile::Standard);
        if cap < evidence_cap {
            evidence_cap = cap;
        }
    }

    if let Some(asr) = &bench.asr {
        if asr.error.is_some() || asr.rtf > ASR_RTF_MAX_VIABLE {
            speech_viable = false;
            notes.push("Transcription could not keep up on this computer during the test.".into());
        } else if asr.rtf > ASR_RTF_REALTIME_TIGHT {
            realtime = false;
            evidence_cap = evidence_cap.min(CapabilityProfile::Basic);
            notes.push("Live transcript is turned off; the transcript will be ready shortly after the meeting.".into());
        } else if asr.rtf > ASR_RTF_REALTIME_COMFORTABLE {
            evidence_cap = evidence_cap.min(CapabilityProfile::Standard);
        }
    }

    let small_bench = bench.llm_for(&small.id);
    let large_bench = bench.llm_for(&large.id);

    if let Some(b) = small_bench {
        if !b.stable || !b.structured_output_valid || b.generation_tokens_per_second < LLM_TPS_BASIC_MIN {
            small_llm_ok = false;
            large_llm_ok = false;
            evidence_cap = evidence_cap.min(CapabilityProfile::Basic);
            notes.push("Local meeting summaries did not pass the reliability test on this computer.".into());
        } else {
            let tier = tier_for_llm_tps(b.generation_tokens_per_second);
            // A real benchmark may lift the acceleration cap (promotion).
            let estimated_large = estimate_generation_tps(small, b.generation_tokens_per_second, large);
            let tier = if tier == CapabilityProfile::Full && estimated_large < LARGE_LLM_MIN_GEN_TPS {
                CapabilityProfile::Enhanced
            } else {
                tier
            };
            if tier > evidence_cap && large_bench.is_none() {
                adjustments.push(ProfileAdjustment {
                    from: evidence_cap,
                    to: tier,
                    reason: format!(
                        "Local AI ran at {:.0} tokens/s, faster than the hardware alone suggested.",
                        b.generation_tokens_per_second
                    ),
                });
            }
            evidence_cap = if large_bench.is_none() { tier } else { evidence_cap.min(tier).max(evidence_cap) };
        }
    }

    if let Some(b) = large_bench {
        let ok = b.stable
            && b.structured_output_valid
            && b.generation_tokens_per_second >= LARGE_LLM_MIN_GEN_TPS
            && b.prompt_tokens_per_second >= LARGE_LLM_MIN_PROMPT_TPS;
        if ok {
            evidence_cap = CapabilityProfile::Full;
        } else {
            large_llm_ok = false;
            if evidence_cap > CapabilityProfile::Enhanced || ceiling == Some(CapabilityProfile::Full) {
                adjustments.push(ProfileAdjustment {
                    from: CapabilityProfile::Full,
                    to: CapabilityProfile::Enhanced,
                    reason: format!(
                        "The large model ran at {:.1} tokens/s, too slow for comfortable use.",
                        b.generation_tokens_per_second
                    ),
                });
            }
            evidence_cap = evidence_cap.min(CapabilityProfile::Enhanced);
        }
    }

    // --- Combine ----------------------------------------------------------
    let profile = match ceiling {
        Some(c) => c.min(evidence_cap),
        None => CapabilityProfile::Basic,
    };
    if let (Some(c), Some(note)) = (ceiling, &accel_note) {
        if accel_cap < c && evidence_cap == accel_cap {
            adjustments.push(ProfileAdjustment { from: c, to: profile, reason: note.clone() });
        }
    }
    if ceiling.is_none() {
        notes.push("There is not enough free memory for transcription models.".into());
    }

    // --- Concrete settings --------------------------------------------------
    let speech_ws = catalog::speech_working_set_bytes(diarization_fits);
    let pick = |spec: &LlmSpec, ctxs: &[u32], concurrent: bool| {
        best_context(&budget, BudgetBasis::Planning, &spec.footprint, speech_ws, ctxs, concurrent)
    };
    let (llm, fit) = match profile {
        CapabilityProfile::Full if large_llm_ok => (Some(large), pick(large, FULL_CONTEXTS, false)),
        CapabilityProfile::Full | CapabilityProfile::Enhanced => (Some(small), pick(small, ENHANCED_CONTEXTS, true)),
        CapabilityProfile::Standard => (Some(small), pick(small, STANDARD_CONTEXTS, true)),
        CapabilityProfile::Basic if small_llm_ok => (Some(small), pick(small, BASIC_CONTEXTS, false)),
        CapabilityProfile::Basic => (None, None),
    };
    let (llm, fit) = match (llm, fit) {
        (Some(l), Some(f)) => (Some(l), Some(f)),
        _ => (None, None),
    };
    if llm.is_none() && speech_viable {
        notes.push("Meeting summaries are not available on this computer. Recording and transcripts still work.".into());
    }

    let physical = hw.cpu.physical_cores.unwrap_or(hw.cpu.logical_cores / 2).max(1);
    let diarization_threads = match profile {
        CapabilityProfile::Basic => 1,
        CapabilityProfile::Standard => 2,
        _ => 4,
    }
    .min(physical);
    let settings = ProfileSettings {
        llm_model_id: llm.map(|l| l.id.clone()),
        context_tokens: fit.as_ref().map(|f| f.context_tokens).unwrap_or(0),
        sequential_loading: fit.as_ref().is_some_and(|f| f.mode == FitMode::Sequential),
        keep_llm_resident: fit.as_ref().is_some_and(|f| f.mode == FitMode::Concurrent)
            && profile >= CapabilityProfile::Enhanced,
        llm_fit: fit,
        realtime_transcript: realtime && speech_viable,
        diarization: diarization_fits,
        diarization_threads,
        asr_threads: (physical / 2).clamp(1, 4),
    };

    // --- Presentation --------------------------------------------------------
    let asr_rating = match &bench.asr {
        Some(a) if a.error.is_none() => match a.rtf {
            r if r < 0.1 => Rating::Excellent,
            r if r < 0.3 => Rating::Good,
            r if r < ASR_RTF_REALTIME_TIGHT => Rating::Standard,
            r if r <= ASR_RTF_MAX_VIABLE => Rating::Limited,
            _ => Rating::Unavailable,
        },
        Some(_) => Rating::Unavailable,
        None if !speech_viable => Rating::Unavailable,
        None => match profile {
            CapabilityProfile::Full | CapabilityProfile::Enhanced => Rating::Excellent,
            CapabilityProfile::Standard => Rating::Good,
            CapabilityProfile::Basic => Rating::Standard,
        },
    };
    let speaker_rating = if !diarization_fits || !speech_viable {
        Rating::Unavailable
    } else if profile == CapabilityProfile::Basic {
        asr_rating.min(Rating::Standard)
    } else {
        asr_rating.min(Rating::Excellent)
    };
    let summary_rating = match (llm, profile) {
        (None, _) => Rating::Unavailable,
        (Some(_), CapabilityProfile::Full) if large_llm_ok => Rating::Excellent,
        (Some(_), CapabilityProfile::Full | CapabilityProfile::Enhanced) => Rating::Good,
        (Some(_), CapabilityProfile::Standard) => Rating::Standard,
        (Some(_), CapabilityProfile::Basic) => Rating::Limited,
    };

    let headline = if !speech_viable {
        Headline::NotSupported
    } else if requirements.level != SupportLevel::Supported || profile == CapabilityProfile::Basic {
        Headline::Limited
    } else if profile >= CapabilityProfile::Enhanced {
        Headline::Ready
    } else {
        Headline::Compatible
    };

    let mut required_models = catalog::speech_model_ids(diarization_fits);
    if let Some(l) = llm {
        required_models.push(l.id.clone());
    }

    let large_fits = assess_llm_fit(&budget, BudgetBasis::Planning, &large.footprint, FULL_CONTEXTS[1], speech_ws).fits();
    let inference = InferenceSupport {
        asr_supported: speech_viable,
        diarization_supported: diarization_fits && speech_viable,
        large_llm_supported: large_llm_ok && large_fits && profile == CapabilityProfile::Full,
        small_llm_supported: small_llm_ok && llm.is_some(),
        recommended_profile: profile,
        llm_backend: llm_backend(hw).into(),
    };

    // Only a failed speech benchmark blocks the user; everything else warns.
    let can_continue = !(bench.asr.is_some() && !speech_viable);

    CapabilityAssessment {
        requirements,
        budget,
        memory_ceiling: ceiling,
        acceleration_cap: accel_cap,
        profile,
        basis,
        adjustments,
        settings,
        inference,
        ratings: Ratings {
            transcription: asr_rating,
            speaker_recognition: speaker_rating,
            meeting_summaries: summary_rating,
            estimated: basis != AssessmentBasis::Benchmarked,
        },
        headline,
        can_continue,
        notes,
        required_models,
    }
}

/// Decision made right before loading the LLM, using *live* memory. May pick
/// a smaller context or model than the profile, or refuse with a warning.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LivePlan {
    pub llm_model_id: Option<String>,
    pub context_tokens: u32,
    pub unload_speech_first: bool,
    /// Plain-language warning if we had to downgrade or cannot proceed.
    pub warning: Option<String>,
}

pub fn plan_for_live_conditions(
    hw: &HardwareSnapshot,
    preferred_model: &LlmSpec,
    preferred_context: u32,
    speech_loaded: bool,
) -> LivePlan {
    let budget = MemoryBudget::compute(&budget_input(hw));
    let speech_ws = if speech_loaded { catalog::speech_working_set_bytes(true) } else { 0 };
    let small = catalog::small_llm();
    let mut candidates: Vec<(&LlmSpec, u32)> = vec![(preferred_model, preferred_context)];
    for ctx in [16384u32, 8192, 4096] {
        if ctx < preferred_context {
            candidates.push((preferred_model, ctx));
        }
    }
    if preferred_model.id != small.id {
        for ctx in [16384u32, 8192, 4096] {
            candidates.push((small, ctx));
        }
    }
    for (i, (spec, ctx)) in candidates.iter().enumerate() {
        let fit = assess_llm_fit(&budget, BudgetBasis::Live, &spec.footprint, *ctx, speech_ws);
        if fit.mode == FitMode::DoesNotFit {
            continue;
        }
        let warning = (i > 0).then(|| {
            "Your computer is low on free memory, so a lighter summary setting is being used. Closing other apps can \
             help."
                .to_string()
        });
        return LivePlan {
            llm_model_id: Some(spec.id.clone()),
            context_tokens: *ctx,
            unload_speech_first: fit.mode == FitMode::Sequential,
            warning,
        };
    }
    LivePlan {
        llm_model_id: None,
        context_tokens: 0,
        unload_speech_first: true,
        warning: Some(
            "There is not enough free memory to create the summary right now. Your transcript is saved; close other \
             apps and try again."
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::benchmark::{AsrBenchmark, LlmBenchmark};
    use crate::system::fixtures;

    #[test]
    fn fixtures_match_expected_profiles() {
        for f in fixtures::all() {
            let a = assess(&f.hardware, None);
            assert_eq!(a.profile, f.expected.profile, "fixture {} profile (ceiling {:?}, cap {:?})", f.id, a.memory_ceiling, a.acceleration_cap);
            assert_eq!(a.settings.llm_model_id, f.expected.llm_model_id, "fixture {} model", f.id);
            if let Some(bench) = &f.benchmark {
                let b = assess(&f.hardware, Some(bench));
                if let Some(p) = f.expected.profile_after_benchmark {
                    assert_eq!(b.profile, p, "fixture {} after benchmark", f.id);
                }
                if let Some(m) = &f.expected.llm_model_id_after_benchmark {
                    assert_eq!(b.settings.llm_model_id.as_ref(), Some(m), "fixture {} model after benchmark", f.id);
                }
            }
        }
    }

    #[test]
    fn eight_gb_is_limited_but_usable() {
        let a = assess(&fixtures::load("win-intel-8gb").hardware, None);
        assert_eq!(a.headline, Headline::Limited);
        assert!(a.can_continue);
        assert!(a.inference.asr_supported);
        assert!(!a.inference.large_llm_supported);
        assert!(a.settings.sequential_loading, "8 GB must load models sequentially");
    }

    #[test]
    fn ram_alone_does_not_decide_full() {
        // 32 GB CPU-only is not Full without evidence; 32 GB with a GPU is.
        let cpu = assess(&fixtures::load("win-32gb-cpu-only").hardware, None);
        let gpu = assess(&fixtures::load("win-32gb-rtx-12gb").hardware, None);
        assert_eq!(cpu.memory_ceiling, Some(CapabilityProfile::Full));
        assert!(cpu.profile < CapabilityProfile::Full);
        assert_eq!(gpu.profile, CapabilityProfile::Full);
    }

    #[test]
    fn gpu_machine_with_less_ram_can_beat_cpu_machine_with_more() {
        let gpu16 = assess(&fixtures::load("win-16gb-rtx-8gb").hardware, None);
        let cpu16 = assess(&fixtures::load("win-cpu-16gb").hardware, None);
        assert!(gpu16.profile > cpu16.profile);
    }

    #[test]
    fn slow_large_model_demotes_full_to_enhanced() {
        let hw = fixtures::load("mac-32gb").hardware;
        let bench = BenchmarkSummary {
            llm: vec![LlmBenchmark {
                model_id: catalog::large_llm().id.clone(),
                generation_tokens_per_second: 3.0,
                prompt_tokens_per_second: 40.0,
                stable: true,
                structured_output_valid: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let a = assess(&hw, Some(&bench));
        assert_eq!(a.profile, CapabilityProfile::Enhanced);
        assert_eq!(a.settings.llm_model_id.as_deref(), Some(catalog::small_llm().id.as_str()));
        assert!(!a.adjustments.is_empty());
    }

    #[test]
    fn fast_small_model_promotes_cpu_only_machine() {
        let hw = fixtures::load("win-32gb-cpu-only").hardware;
        let before = assess(&hw, None).profile;
        let bench = BenchmarkSummary {
            llm: vec![LlmBenchmark {
                model_id: catalog::small_llm().id.clone(),
                generation_tokens_per_second: 45.0,
                prompt_tokens_per_second: 300.0,
                stable: true,
                structured_output_valid: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let after = assess(&hw, Some(&bench));
        assert!(after.profile > before, "{:?} -> {:?}", before, after.profile);
        assert_eq!(after.profile, CapabilityProfile::Full);
    }

    #[test]
    fn slow_asr_disables_realtime_and_caps_basic() {
        let hw = fixtures::load("mac-m1-16gb").hardware;
        let bench = BenchmarkSummary {
            asr: Some(AsrBenchmark { rtf: 0.9, output_plausible: true, ..Default::default() }),
            ..Default::default()
        };
        let a = assess(&hw, Some(&bench));
        assert_eq!(a.profile, CapabilityProfile::Basic);
        assert!(!a.settings.realtime_transcript);
        assert!(a.can_continue);
    }

    #[test]
    fn failed_speech_benchmark_blocks() {
        let hw = fixtures::load("win-intel-8gb").hardware;
        let bench = BenchmarkSummary {
            asr: Some(AsrBenchmark { rtf: 6.0, ..Default::default() }),
            ..Default::default()
        };
        let a = assess(&hw, Some(&bench));
        assert!(!a.can_continue);
        assert_eq!(a.headline, Headline::NotSupported);
    }

    #[test]
    fn invalid_structured_output_disables_summaries_on_basic() {
        let hw = fixtures::load("win-intel-8gb").hardware;
        let bench = BenchmarkSummary {
            llm: vec![LlmBenchmark {
                model_id: catalog::small_llm().id.clone(),
                generation_tokens_per_second: 6.0,
                stable: true,
                structured_output_valid: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let a = assess(&hw, Some(&bench));
        assert_eq!(a.settings.llm_model_id, None);
        assert_eq!(a.ratings.meeting_summaries, Rating::Unavailable);
    }

    #[test]
    fn memory_pressure_downgrades_live_plan() {
        let mut hw = fixtures::load("mac-m1-16gb").hardware;
        let small = catalog::small_llm();
        let relaxed = plan_for_live_conditions(&hw, small, 16384, true);
        assert!(relaxed.warning.is_none(), "{relaxed:?}");
        hw.memory.available_bytes = 4 * GIB;
        hw.memory.pressure = Some(super::super::memory_budget::MemoryPressure::High);
        let tight = plan_for_live_conditions(&hw, small, 16384, true);
        assert!(tight.warning.is_some());
        assert!(tight.context_tokens < 16384 || tight.llm_model_id.is_none() || tight.unload_speech_first);
    }

    #[test]
    fn live_plan_refuses_when_nothing_fits() {
        let mut hw = fixtures::load("win-intel-8gb").hardware;
        hw.memory.available_bytes = GIB;
        let plan = plan_for_live_conditions(&hw, catalog::small_llm(), 8192, false);
        assert!(plan.llm_model_id.is_none());
        assert!(plan.warning.is_some());
    }
}
