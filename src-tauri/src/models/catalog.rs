//! Static model catalog. Every URL, size and SHA-256 here was verified
//! against the hosting API (see docs/models.md for provenance).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::system::memory_budget::{MemoryFootprint, GIB, KIB, MIB};
use crate::system::profile::CapabilityProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ModelPurpose {
    Asr,
    Vad,
    Diarization,
    Embedding,
    Llm,
}

impl ModelPurpose {
    pub fn as_db(self) -> &'static str {
        match self {
            ModelPurpose::Asr => "asr",
            ModelPurpose::Vad => "vad",
            ModelPurpose::Diarization => "diarization",
            ModelPurpose::Embedding => "embedding",
            ModelPurpose::Llm => "llm",
        }
    }
}

/// One downloadable file of a model.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelFile {
    /// Path relative to the model's install directory.
    pub path: String,
    pub download_url: String,
    pub byte_size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelRequirements {
    /// Lowest capability profile this model is recommended for.
    pub minimum_profile: Option<CapabilityProfile>,
}

/// Manifest for a downloadable model. Speech models consist of several
/// files, so `files` generalises the single `downloadUrl`/`sha256` pair; the
/// aggregate `byte_size` is the sum of the files.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelManifest {
    pub id: String,
    pub name: String,
    pub purpose: ModelPurpose,
    pub version: String,
    pub byte_size: u64,
    pub files: Vec<ModelFile>,
    pub license: String,
    pub license_url: String,
    pub minimum_capabilities: Option<ModelRequirements>,
}

/// Runtime characteristics of an LLM used for budgeting and estimates.
#[derive(Debug, Clone)]
pub struct LlmSpec {
    pub id: String,
    pub footprint: MemoryFootprint,
    pub total_params_billions: f32,
    pub active_params_billions: f32,
    pub mixture_of_experts: bool,
    pub max_context: u32,
}

impl LlmSpec {
    /// Bytes read per generated token (active weights only for MoE).
    pub fn active_weight_bytes(&self) -> u64 {
        (self.footprint.weights_bytes as f64 * (self.active_params_billions / self.total_params_billions) as f64)
            as u64
    }
}

pub const LLM_SMALL_ID: &str = "nemotron-3-nano-4b-q4km";
pub const LLM_LARGE_ID: &str = "nemotron-3-nano-30b-a3b-q4km";
pub const ASR_ID: &str = "parakeet-tdt-0.6b-v3-int8";
pub const VAD_ID: &str = "silero-vad";
pub const DIARIZATION_ID: &str = "nemotron-3-diarization";
pub const EMBEDDING_ID: &str = "speaker-embedding";

const NEMOTRON_LICENSE: &str = "NVIDIA Nemotron Open Model License";
const NEMOTRON_LICENSE_URL: &str =
    "https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-nemotron-open-model-license/";

fn hf(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

fn llm_specs() -> &'static [LlmSpec] {
    static SPECS: OnceLock<Vec<LlmSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        vec![
            // Dense hybrid Mamba-2/Transformer, 4 attention layers × 8 KV heads × 128.
            // KV 16 KiB/token and 81 MiB recurrent state measured in llama.cpp b11193;
            // ~215 MiB compute buffer at 32k plus ~200 MiB runtime.
            LlmSpec {
                id: LLM_SMALL_ID.into(),
                footprint: MemoryFootprint {
                    weights_bytes: 2_837_072_864,
                    kv_bytes_per_token: 16 * KIB,
                    fixed_overhead_bytes: 500 * MIB,
                },
                total_params_billions: 3.97,
                active_params_billions: 3.97,
                mixture_of_experts: false,
                max_context: 262_144,
            },
            // Hybrid MoE, 6 attention layers × 2 KV heads × 128 → 6 KiB/token.
            // ~48 MiB recurrent state, few hundred MiB compute buffer + runtime.
            LlmSpec {
                id: LLM_LARGE_ID.into(),
                footprint: MemoryFootprint {
                    weights_bytes: 22_421_827_488,
                    kv_bytes_per_token: 6 * KIB,
                    fixed_overhead_bytes: 560 * MIB,
                },
                total_params_billions: 31.6,
                active_params_billions: 3.5,
                mixture_of_experts: true,
                max_context: 262_144,
            },
        ]
    })
}

pub fn llm_spec(id: &str) -> Option<&'static LlmSpec> {
    llm_specs().iter().find(|s| s.id == id)
}

pub fn small_llm() -> &'static LlmSpec {
    llm_spec(LLM_SMALL_ID).expect("small LLM in catalog")
}

pub fn large_llm() -> &'static LlmSpec {
    llm_spec(LLM_LARGE_ID).expect("large LLM in catalog")
}

/// Estimated resident memory of the speech stack during a meeting.
/// ASR ≈ int8 weights + ONNX Runtime arenas; diarization = segmentation +
/// embedding models + clustering buffers. Refined by measured peak RSS.
pub const ASR_WORKING_SET: u64 = 1024 * MIB;
pub const VAD_WORKING_SET: u64 = 32 * MIB;
/// Nemotron 3 Diarization q8_0 (107 MB) plus runtime buffers, in its own
/// process; conservative until measured on long meetings.
pub const DIARIZATION_WORKING_SET: u64 = 512 * MIB;

pub fn speech_working_set_bytes(with_diarization: bool) -> u64 {
    ASR_WORKING_SET + VAD_WORKING_SET + if with_diarization { DIARIZATION_WORKING_SET } else { 0 }
}

pub fn speech_model_ids(with_diarization: bool) -> Vec<String> {
    let mut ids = vec![VAD_ID.to_string(), ASR_ID.to_string()];
    if with_diarization {
        ids.push(DIARIZATION_ID.to_string());
    }
    // The embedding model is needed for diarization *and* voice enrollment.
    ids.push(EMBEDDING_ID.to_string());
    ids
}

fn manifest(
    id: &str,
    name: &str,
    purpose: ModelPurpose,
    version: &str,
    files: Vec<ModelFile>,
    license: &str,
    license_url: &str,
    minimum_profile: Option<CapabilityProfile>,
) -> ModelManifest {
    ModelManifest {
        id: id.into(),
        name: name.into(),
        purpose,
        version: version.into(),
        byte_size: files.iter().map(|f| f.byte_size).sum(),
        files,
        license: license.into(),
        license_url: license_url.into(),
        minimum_capabilities: minimum_profile.map(|p| ModelRequirements { minimum_profile: Some(p) }),
    }
}

fn file(path: &str, url: String, byte_size: u64, sha256: &str) -> ModelFile {
    ModelFile { path: path.into(), download_url: url, byte_size, sha256: sha256.into() }
}

pub fn manifests() -> &'static [ModelManifest] {
    static M: OnceLock<Vec<ModelManifest>> = OnceLock::new();
    M.get_or_init(|| {
        let mut v = super::catalog_speech::speech_manifests();
        v.push(manifest(
            LLM_SMALL_ID,
            "Nemotron 3 Nano 4B",
            ModelPurpose::Llm,
            "Q4_K_M",
            vec![file(
                "NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf",
                hf("nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF", "NVIDIA-Nemotron3-Nano-4B-Q4_K_M.gguf"),
                2_837_072_864,
                "be5d9a656a51922f24f1f09a759cebb694e1f5d9728bf0ef9f8c972c5a0b5ef2",
            )],
            NEMOTRON_LICENSE,
            NEMOTRON_LICENSE_URL,
            Some(CapabilityProfile::Basic),
        ));
        v.push(manifest(
            LLM_LARGE_ID,
            "Nemotron 3 Nano 30B-A3B",
            ModelPurpose::Llm,
            "Q4_K_M",
            vec![file(
                "NVIDIA-Nemotron-3-Nano-30B-A3B-Q4_K_M.gguf",
                hf("ggml-org/NVIDIA-Nemotron-3-Nano-30B-A3B-GGUF", "NVIDIA-Nemotron-3-Nano-30B-A3B-Q4_K_M.gguf"),
                22_421_827_488,
                "0f111a0d49777a2a0178758b1aee14e5365c495987499401ef3ebe87754322a8",
            )],
            NEMOTRON_LICENSE,
            NEMOTRON_LICENSE_URL,
            Some(CapabilityProfile::Full),
        ));
        v
    })
}

pub fn manifest_by_id(id: &str) -> Option<&'static ModelManifest> {
    manifests().iter().find(|m| m.id == id)
}

/// Rough total download for a set of models.
pub fn download_bytes(ids: &[String]) -> u64 {
    ids.iter().filter_map(|id| manifest_by_id(id)).map(|m| m.byte_size).sum()
}

#[allow(dead_code)]
const _: u64 = GIB;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_manifest_is_complete() {
        for m in manifests() {
            assert!(!m.files.is_empty(), "{}", m.id);
            for f in &m.files {
                assert!(f.download_url.starts_with("https://"), "{} {}", m.id, f.path);
                assert_eq!(f.sha256.len(), 64, "{} {} sha256", m.id, f.path);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(f.byte_size > 0);
                assert!(!f.path.contains(".."));
            }
            assert_eq!(m.byte_size, m.files.iter().map(|f| f.byte_size).sum::<u64>());
        }
    }

    #[test]
    fn required_ids_exist_in_catalog() {
        for id in speech_model_ids(true).iter().chain([LLM_SMALL_ID.to_string(), LLM_LARGE_ID.to_string()].iter()) {
            assert!(manifest_by_id(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn moe_active_weights_are_smaller_than_total() {
        let l = large_llm();
        assert!(l.active_weight_bytes() < l.footprint.weights_bytes / 5);
        let s = small_llm();
        assert_eq!(s.active_weight_bytes(), s.footprint.weights_bytes);
    }
}
