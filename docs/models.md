# Models

No model weights ship in the installer. The model manager downloads them only when you click to download:
- Each file resumes over HTTP Range requests.
- Each file is checked against its SHA-256.
- Files are renamed into place only once verified.
- Downloads can be paused, cancelled or retried.
- Disk space is checked first, keeping a 3 GB working reserve plus 5% of the download.
- Models can be removed, and the models folder can be moved (Settings → Storage).

Advanced users can import a custom GGUF file. The file's GGUF header is checked, and its memory needs are estimated conservatively from its size.

## Catalog (verified 2026-09-26)

| Id | Purpose | Source | Size | License |
|---|---|---|---|---|
| `silero-vad` | voice activity | k2-fsa/sherpa-onnx release `asr-models/silero_vad.onnx` | 0.6 MB | MIT |
| `parakeet-tdt-0.6b-v3-int8` | ASR (multilingual, 25 European languages) | `csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` | 670 MB | CC-BY-4.0 |
| `nemotron-3-diarization` | diarization (streaming Sortformer, up to 8 speakers) | `nvidia/Nemotron-3-Diarization` (q8_0 GGUF) | 107 MB | OpenMDW-1.1 |
| `speaker-embedding` | voiceprints (TitaNet small) | `csukuangfj/speaker-embedding-models` | 40 MB | CC-BY-4.0 |
| `nemotron-3-nano-4b-q4km` | summaries (Standard/Enhanced/Basic) | `nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF` | 2.84 GB | NVIDIA Nemotron Open Model License |
| `nemotron-3-nano-30b-a3b-q4km` | summaries (Full) | `ggml-org/NVIDIA-Nemotron-3-Nano-30B-A3B-GGUF` | 22.4 GB | NVIDIA Nemotron Open Model License |

Exact URLs and hashes are in `src-tauri/src/models/catalog*.rs`. NVIDIA publishes no official 30B GGUF; the ggml-org build comes from the llama.cpp maintainers.

Diarization runs in NVIDIA's NeMo-Speech.cpp (Apache-2.0, C++/ggml), bundled at `src-tauri/binaries/nemo-speech/`. It is built from pinned commit `97a15af` by `scripts/build-nemo-speech.mjs`, because the only published release (v0.1.0) predates Nemotron 3 Diarization support and fails with "pre_ln transformer variant is not supported". macOS uses Metal; Windows uses the CPU build. SentencePiece is linked statically so the runtime has no Homebrew dependencies.

The llama.cpp runtime is pinned to `b11193` with SHA-256 by `scripts/fetch-llama.mjs`:
- macOS arm64 uses the Metal build.
- Windows x64 uses the Vulkan build, which also contains the CPU backends and falls back to them when no Vulkan device exists.

## Memory characteristics used for budgeting

| Model | Weights | KV cache | Fixed |
|---|---|---|---|
| Nemotron 3 Nano 4B | 2.64 GiB | 16 KiB/token (4 attention layers × 8 KV heads) | ~500 MiB (81 MiB recurrent state + compute buffers) |
| Nemotron 3 Nano 30B-A3B | 20.9 GiB | 6 KiB/token (6 attention layers × 2 KV heads) | ~560 MiB |
| Speech stack | ASR ~1 GiB (measured RSS ≈1.19 GB incl. app), VAD 32 MiB, diarization ~384 MiB | | |

The 4B figures were measured with llama.cpp b11193. The 30B figures were computed from its config with the same formula that reproduced the 4B measurements.

Both Nemotron models "think" by default. The app disables this: `--reasoning off` on the server and `enable_thinking: false` per request. Reasoning is never shown or stored.

## Calibration notes

These are starting points, adjustable in Settings → Advanced. They are not universal truths.

- **Diarization** uses Nemotron 3 Diarization. It assigns speakers in order of first appearance and handles overlapping speech, with no clustering threshold to tune.
  - On the two-speaker sample it found both speakers in 0.6 s (warm, Metal) for 27 s of audio. The first run compiles Metal shaders (~20 s once). CPU: ~3 s.
  - It also attributed a short reply correctly that the previous pyannote + clustering pipeline got wrong.
  - It replaced pyannote segmentation + WeSpeaker/TitaNet clustering, which was only correct at a narrow threshold range.
- **Voice matching.** Per-turn cosine similarity with TitaNet-small was 0.51–0.74 for the same speaker and 0.09–0.36 for different speakers.
  - Thresholds: **known ≥ 0.60** (with a 0.08 margin over the runner-up) and **possible ≥ 0.45**.
  - Enrollment keeps windows with similarity ≥ 0.60 to the profile centroid and requires ≥ 0.45 mean pairwise similarity.
  - In the end-to-end test, an enrolled speaker was recognised as "known" at 0.85.
- These values come from a single recording. Calibrate on your own meetings before relying on automatic names. "Possible" matches are never used as names in notes or actions until a person confirms them.

## Measured performance (M4 Max, 48 GB)

- **Parakeet (int8, CPU, 4 threads):** load 0.67 s, RTF 0.066, ~1.2 GB resident.
- **Nemotron 4B Q4_K_M (Metal):** loads in about 1 s. Structured analysis of a 7-line transcript took 9–14 s including one validation retry. The research run measured about 28 tokens/s generation.
