//! Speech model manifests (sherpa-onnx exports). Sizes and SHA-256 values
//! come from the HuggingFace tree API (`lfs.oid`) or GitHub release asset
//! digests, cross-checked by hashing downloaded copies.

use super::catalog::{ModelFile, ModelManifest, ModelPurpose, ASR_ID, EMBEDDING_ID, DIARIZATION_ID, VAD_ID};

fn hf(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

fn f(path: &str, url: String, byte_size: u64, sha256: &str) -> ModelFile {
    ModelFile { path: path.into(), download_url: url, byte_size, sha256: sha256.into() }
}

fn m(id: &str, name: &str, purpose: ModelPurpose, version: &str, files: Vec<ModelFile>, license: &str, url: &str) -> ModelManifest {
    ModelManifest {
        id: id.into(),
        name: name.into(),
        purpose,
        version: version.into(),
        byte_size: files.iter().map(|f| f.byte_size).sum(),
        files,
        license: license.into(),
        license_url: url.into(),
        minimum_capabilities: None,
    }
}

pub fn speech_manifests() -> Vec<ModelManifest> {
    const PARAKEET: &str = "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
    vec![
        m(
            VAD_ID,
            "Silero VAD",
            ModelPurpose::Vad,
            "v5",
            vec![f(
                "silero_vad.onnx",
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx".into(),
                643_854,
                "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
            )],
            "MIT",
            "https://github.com/snakers4/silero-vad/blob/master/LICENSE",
        ),
        m(
            ASR_ID,
            "Parakeet TDT 0.6B v3",
            ModelPurpose::Asr,
            "v3-int8",
            vec![
                f("encoder.int8.onnx", hf(PARAKEET, "encoder.int8.onnx"), 652_184_281,
                  "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247"),
                f("decoder.int8.onnx", hf(PARAKEET, "decoder.int8.onnx"), 11_845_275,
                  "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e"),
                f("joiner.int8.onnx", hf(PARAKEET, "joiner.int8.onnx"), 6_355_277,
                  "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3"),
                f("tokens.txt", hf(PARAKEET, "tokens.txt"), 93_939,
                  "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
                // Short English sample used by the transcription benchmark.
                f("test_wavs/en.wav", hf(PARAKEET, "test_wavs/en.wav"), 184_608,
                  "148b936b43ce7c546a866e64da059f0458aee2d65e617f16e9d94f06e8d99ed6"),
            ],
            "CC-BY-4.0",
            "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
        ),
        m(
            DIARIZATION_ID,
            "Nemotron 3 Diarization",
            ModelPurpose::Diarization,
            "q8_0",
            // Streaming Sortformer, up to 8 speakers; run by NeMo-Speech.cpp.
            vec![f(
                "Nemotron-3-Diarization.q8_0.gguf",
                hf("nvidia/Nemotron-3-Diarization", "Nemotron-3-Diarization.q8_0.gguf"),
                107_012_128,
                "08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1",
            )],
            "OpenMDW-1.1",
            "https://openmdw.ai/license/1-1/",
        ),
        m(
            EMBEDDING_ID,
            "TitaNet Small (speaker embeddings)",
            ModelPurpose::Embedding,
            "titanet-small",
            // Chosen over WeSpeaker ResNet34 after calibration: a stable
            // 2-speaker result over thresholds 0.55–0.75 on a two-speaker
            // sample, where ResNet34 was correct only at exactly 0.42.
            vec![f(
                "nemo_en_titanet_small.onnx",
                hf("csukuangfj/speaker-embedding-models", "nemo_en_titanet_small.onnx"),
                40_257_283,
                "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e",
            )],
            "CC-BY-4.0",
            "https://huggingface.co/nvidia/speakerverification_en_titanet_large",
        ),
    ]
}
