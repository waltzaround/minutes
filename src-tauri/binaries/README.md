# Bundled runtimes (not committed)

These folders are generated and bundled as app resources:

- `llama/`: llama.cpp `llama-server`, pinned and SHA-256 verified by `node scripts/fetch-llama.mjs`.
- `nemo-speech/`: NeMo-Speech.cpp diarization runtime, built from a pinned commit by `node scripts/build-nemo-speech.mjs`.
- `ffmpeg/`: standalone FFmpeg decoder, built from official `8.0.1` source with SHA-256 verification by `node scripts/build-ffmpeg.mjs` (macOS Apple Silicon) or `node scripts/build-ffmpeg.mjs win-x64` (Windows x64, MSYS2 MINGW64 with GCC/Make). No external codec libraries; LGPL license, corresponding source archive and recipe are included. Downloads and compilation happen only during development/packaging; imports use this included executable.
- `.cache/`: download and build cache.
