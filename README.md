# Minutes

Minutes is a private, local-first meeting intelligence app for macOS and Windows.

It records your meetings and writes the transcript. It recognises who spoke, then drafts notes, decisions and action items with evidence you can click through to. Every step runs on your own computer: there is no cloud transcription, no cloud LLM and no subscription.

Notion and Linear are optional. Nothing is sent to them until you review it and ask.

- Tauri 2 with a Rust backend and a React/TypeScript/Tailwind/shadcn UI.
- Speech runs on sherpa-onnx (Parakeet TDT 0.6B v3, Silero VAD, TitaNet speaker embeddings) and NVIDIA Nemotron 3 Diarization via NeMo-Speech.cpp.
- Summaries run on llama.cpp (`llama-server` sidecar) with NVIDIA Nemotron 3 Nano 4B or 30B-A3B.
- Data is stored in SQLite with migrations. Secrets live in the macOS Keychain or Windows Credential Manager, and voiceprints are encrypted at rest.

See [docs/status.md](docs/status.md) for what is implemented and verified, and what is not yet.

## Requirements

| | Minimum (supported) | Recommended | Full local AI (30B model) |
|---|---|---|---|
| macOS | 14.6+, Apple Silicon M1+, 16 GB | M2 Pro+, 24–32 GB | 32 GB+ Pro/Max |
| Windows | 10 64-bit, 4+ cores (i5 8th gen / Ryzen 5 3000+), 16 GB | 32 GB, discrete GPU | 32 GB+ and/or large-VRAM GPU |
| Disk | 10 GB free | 30 GB free | 30 GB free |

8 GB machines run in a limited Basic mode. A GPU is never required. See [docs/system-requirements.md](docs/system-requirements.md).

## Development setup

Prerequisites:
- Rust 1.95+ (`rustup update stable`)
- Node 22+ and pnpm 10
- macOS: Xcode command-line tools
- Windows: Visual Studio 2022 Build Tools with the C++ workload, and WebView2 (preinstalled on Windows 11). The prebuilt speech libraries require the newer C++ runtime libraries; Visual Studio 2019 cannot link them.

```bash
pnpm install
```

Download the pinned llama.cpp runtime for this platform. It is SHA-256 verified.

```bash
node scripts/fetch-llama.mjs
```

Build the pinned diarization runtime (NeMo-Speech.cpp, needs CMake and Ninja; on macOS `brew install cmake ninja`):

```bash
node scripts/build-nemo-speech.mjs
```

Run the app in development:

```bash
pnpm tauri dev
```

The first `cargo` build downloads sherpa-onnx's prebuilt static libraries, about 20 MB on macOS and 120 MB on Windows.

### Tests

```bash
pnpm check
```

This runs the TypeScript typecheck, the Vitest tests and the Rust tests. Tests that need real audio hardware or installed models are `#[ignore]`d. Run them locally with:

```bash
cd src-tauri && MINUTES_DATA_DIR="$HOME/Library/Application Support/app.minutes.desktop" cargo test --release -- --ignored --nocapture
```

The main ignored tests:
- `real_device_recording_round_trip`: records the microphone and system audio from real devices.
- `full_pipeline_transcribes_recorded_audio`: runs the full post-meeting pipeline on Parakeet.
- `two_speaker_pipeline`: runs diarization and recognition. It needs `MINUTES_DIARIZATION_WAV`, a two-speaker WAV that is not committed.
- `real_llm_analysis`: runs `llama-server` with Nemotron 4B.
- `asr_benchmark_on_real_model`: runs the transcription benchmark.

### Installing models without the UI

The app downloads models from Settings → Models or during onboarding. For provisioning or development there is a CLI that uses the same verified downloader:

```bash
cd src-tauri && cargo run --example install_models -- "<app data dir>" silero-vad parakeet-tdt-0.6b-v3-int8 nemotron-3-diarization speaker-embedding nemotron-3-nano-4b-q4km
```

### Previewing the UI in a browser

`pnpm dev`, then open <http://localhost:1420/?mock=mac-m1-16gb&onboarded>. This loads an explicit development mock backend: it is dev-only, and a yellow banner is shown. Its capability data is computed by the real Rust assessment for the hardware fixtures in `src-tauri/fixtures/hardware`. Regenerate that data with:

```bash
cd src-tauri && MINUTES_EXPORT_SAMPLES=1 cargo test export_capability_samples
```

### TypeScript types

The Rust types are exported with ts-rs. Regenerate them with:

```bash
pnpm types
```

## Building installers

```bash
pnpm tauri build
```

This produces `src-tauri/target/release/bundle/dmg/*.dmg` on macOS and `…/nsis/*.exe` on Windows. Signing, notarisation and updates are covered in [docs/packaging-macos.md](docs/packaging-macos.md) and [docs/packaging-windows.md](docs/packaging-windows.md).

## Documentation

- [Architecture](docs/architecture.md)
- [Audio capture](docs/audio.md)
- [Models](docs/models.md)
- [System requirements](docs/system-requirements.md)
- [Capability detection and benchmarks](docs/capability-detection.md)
- [Privacy and security](docs/privacy.md)
- [Notion](docs/notion.md) · [Linear](docs/linear.md)
- [Packaging: macOS](docs/packaging-macos.md) · [Windows](docs/packaging-windows.md)
- [Dependency verification log](docs/dependency-verification.md)
- [Implementation status](docs/status.md)
