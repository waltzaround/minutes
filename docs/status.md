# Implementation status (v0.1, 2026-09-26)

This page says honestly what is built, what was verified and how, and what is still open.

## Verification environment

- macOS 26.6 on an Apple M4 Max with 48 GB, rustc 1.98.1, Node 22.
- No Windows machine was available. Windows code is compiled only in CI (`.github/workflows/ci.yml`, not yet run) and has not been exercised on hardware.

## Phases

| Phase | Status | Evidence |
|---|---|---|
| 1. Foundation: Tauri shell, React UI, SQLite + migrations, capability detection, requirements, tiers, memory budget, benchmark framework, permissions, device enumeration | Done | 30+ unit tests; 9 hardware fixtures with expected tiers; real detection on this Mac |
| 2. Recording: mic + system audio, separate streams, levels, disconnect warnings and reconnect, recoverable sessions | Done on macOS | `real_device_recording_round_trip`; WAV header repair test; crash-interrupt test |
| 3. Transcription: Parakeet, VAD, chunking, word timestamps, live transcript, post-meeting gap filling | Done | Real-model tests: ASR, VAD, full pipeline (48 kHz stereo WAV → correctly timed segments), ASR benchmark (RTF 0.066) |
| 4. Speakers: diarization, alignment, enrollment, known-speaker matching, speaker editing (rename/map/merge/split/correct) | Done | Real two-speaker pipeline: 2 clusters, enrolled speaker recognised at 0.85; editing operations unit-tested |
| 5. Local LLM: model manager, llama.cpp sidecar, capability-based selection, sequential loading, structured analysis, evidence, benchmark | Done | Real `llama-server` + Nemotron 4B analysis test (owners, acceptance vs suggestion, dates); validation, retry and "unavailable" unit tests; chunked synthesis test |
| 6. Integrations: Notion, Linear, identity mappings, review-before-write | Done (mock-tested) | Local fake servers: Notion block batching + 429 handling; Linear idempotency after a lost response; not yet run against real workspaces |
| 7. Polish: crash recovery, diagnostics, shortcuts, accessibility basics, packaging | Partial | See below |

Test totals: `cargo test` runs 228 tests, and 9 more hardware/model tests are ignored by default (all of those pass on this Mac). `pnpm test` runs 4 tests. The typecheck is clean.

## Known gaps and next steps

1. **Windows is unverified on hardware.** Run CI, then test on a real machine:
   - WASAPI loopback;
   - DXGI and Vulkan detection;
   - sherpa-onnx static linking with `+crt-static`;
   - llama-server Vulkan/CPU backends;
   - the source-built NeMo-Speech.cpp diarization runtime (CPU build, vcpkg SentencePiece);
   - the Credential Manager.
2. **macOS "System Audio Recording" permission** cannot be queried. The app detects silence heuristically during recording. A "play a test sound" check in onboarding would make this explicit.
3. **Live diarization.** During recording, meeting audio is labelled "Meeting audio". Speaker names arrive after the refinement pass, as the brief allows.
4. **Voice-match thresholds** are calibrated on one recording only (diarization itself now uses Nemotron 3 Diarization, with no threshold to tune) (see [models.md](models.md)). Calibrate them on real internal meetings. Do not commit private recordings.
5. **Audio fixture set.** The brief asks for fixtures covering one speaker, two speakers, overlap, silence, noise, and microphone + remote. Only public samples were used, and they are not committed. Assemble a licensed or synthetic fixture set for CI.
6. **Updater:** the architecture is documented but not registered until signing keys and an endpoint exist.
7. **Signing and notarisation:** the steps are documented. The unsigned DMG builds.
8. **30B model:** its memory figures are computed, not measured (22.4 GB download). The benchmark demotes the tier automatically if it is slow.
9. **Accessibility:** there are keyboard shortcuts, focus rings, ARIA live regions for recording, transcript and warnings, and labelled controls. A full screen-reader audit is still to do.
10. **Notion and Linear** have not been tested against real workspaces. The request shapes follow the current docs (see [dependency-verification.md](dependency-verification.md)).

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| ⌘/Ctrl + Shift + R | Start a meeting, or go to the active recording |
| ⌘/Ctrl + , | Settings |
| ⌘/Ctrl + 1 / 2 | Home / People |
| ⌘/Ctrl + B | Show or hide the sidebar |
| ⌘/Ctrl + K | Search meetings, notes and transcripts |
| Transcript: double-click a line | Edit it |
| Transcript: ⌘/Ctrl + Enter | Save an edit |
| Transcript: Esc | Cancel an edit |
| Transcript: Enter in search | Jump to the next match |
