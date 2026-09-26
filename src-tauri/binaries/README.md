# Bundled runtimes (not committed)

These folders are generated and bundled as app resources:

- `llama/`: llama.cpp `llama-server`, pinned and SHA-256 verified by `node scripts/fetch-llama.mjs`.
- `nemo-speech/`: NeMo-Speech.cpp diarization runtime, built from a pinned commit by `node scripts/build-nemo-speech.mjs`.
- `.cache/`: download and build cache.
