# Architecture

```
                         Tauri 2 application
                                 │
          ┌──────────────────────┴───────────────────────┐
          │                                              │
  React UI (src/)                               Rust backend (src-tauri/src/)
  typed invoke() + events                                │
          │          ┌──────────────┬───────────────┬────┴─────────┬───────────────┐
          │       audio/         speech/          llm/           storage/      integrations/
          │   CPAL capture   sherpa-onnx +      llama-server    SQLite +       Notion REST,
          │   mic + system   NeMo-Speech.cpp:   sidecar,        migrations,    Linear GraphQL
          │   ring buffers   VAD, Parakeet,     schemas,        keychain,
          │   → WAV + 16k    Nemotron diar.,    prompts         AES-GCM
          │                  speaker registry
          │                        │                 │
          └──────────── meetings/ (recorder → processor → speakers → analysis) ─────┘
                                   │
                            system/ (capabilities, requirements, memory budget,
                                     profiles, benchmarks, permissions)
```

## Principles in code

- **Replaceable components behind traits:**
  - `SpeechRecognizer` (Parakeet)
  - `Diarizer`
  - `EmbeddingModel`
  - `LocalLlm` (llama-server sidecar today; embedded bindings later)
  - `SecretStore` (OS keychain; in-memory for tests)
  - `PostTranscriptStep` (speaker refinement plugged into the processor)
- **Narrow Tauri boundary.** The UI can only call the commands registered in `lib.rs`. It never touches devices, files, tokens, SQLite or runtimes directly. Tauri capabilities (`src-tauri/capabilities/default.json`) grant only dialog, open-URL and window-drag/title permissions.
- **Typed contract.** Rust types derive `ts_rs::TS` and are exported to `src/lib/types/generated/` (`pnpm types`). Errors cross the boundary as `{ code, message }`, and messages are user-safe. Internal detail goes to the log only.

## Threads and back-pressure

| Work | Where | Never blocks on |
|---|---|---|
| Audio callback | CPAL thread; only converts samples and pushes to a lock-free SPSC ring buffer (4 s) | anything: overruns drop samples and raise a warning |
| Capture worker | one thread per stream; WAV writes, level meter, resampling | UI, inference: speech blocks use `try_send` |
| Live transcription | one thread; VAD + Parakeet per bounded chunk | capture: its queue is bounded (2 min) and overflow becomes a "gap" that the processor re-transcribes |
| Recording pump | one thread; capture events → UI events + SQLite | audio |
| Post-meeting processor | one thread, one meeting at a time | UI |
| Analysis | one thread; llama-server over localhost HTTP | UI, recording |
| Memory monitor | one thread, every 20 s | — |

## Meeting lifecycle

1. **Start.** The meeting row is created with status `recording`. Microphone and system audio start as separate tracks (`audio_tracks`). Each is a WAV file in its original rate and channel layout, and the header is rewritten every 2 s so a crash loses at most 2 s.
2. **Live.** 16 kHz mono blocks go to VAD → chunker → Parakeet. Segments are stored as provisional and emitted as `transcript_segment` events.
3. **Stop.** Files are finalised and the status becomes `processing`. The processor:
   1. waits for the live worker to drain;
   2. re-transcribes any uncovered audio from the WAVs (streamed, never loaded whole);
   3. runs speaker refinement;
   4. marks the transcript final;
   5. applies audio retention;
   6. releases the speech models.
4. **Analysis.** Queued after processing. It uses a live memory plan (smaller context or model if needed). Speech models are unloaded first on sequential tiers. llama-server is started, and the transcript is analysed in a single pass or in chunks followed by synthesis. The output is validated strictly, with one correction retry, then stored with evidence. Owners are resolved deterministically.
5. **Crash recovery.** On launch, any `recording` meeting becomes `interrupted` and the home screen offers to recover it (WAV headers are repaired) or delete it. Any `processing` meeting is queued again.

## Data model

SQLite with foreign keys and WAL. Tables:
- `meetings`, `audio_tracks`, `transcript_segments`
- `speaker_clusters`, `people`, `speaker_profiles`, `speaker_embeddings`, `meeting_participants`
- `meeting_analysis`, `decisions` (+ `decision_evidence`), `unresolved_questions` (+ `question_evidence`), `action_items` (+ `action_item_evidence`)
- `integration_mappings`, `integration_operations`
- `model_installations`, `benchmark_results`, `settings`

Migrations live in `src-tauri/src/storage/migrations/*.sql` and are tracked with `PRAGMA user_version`. Each migration runs in a transaction. A database written by a newer app version is refused rather than guessed at.

JSON columns are used only for secondary structures: word timings, the summary string list, benchmark detail and settings sections.

Transcript text and speaker attribution are separate columns. Diarization refinement and user corrections change `speaker_cluster_id` / `person_id` and never the text. Text corrections keep `original_text`, and every edit bumps `meetings.transcript_revision` so stale notes are flagged.

## Deviations from the original brief (and why)

- **Model manifests use a `files[]` list** (each file with its own URL and SHA-256), because speech models are several files. `byteSize` is the total.
- **The model catalog lives in `models/`.** `llm/model_manager.rs` handles LLM selection and custom GGUF imports.
- **The 30B model is ~22.4 GB (Q4_K_M), not ~18 GB.** There is no official NVIDIA GGUF, so it comes from ggml-org, the llama.cpp maintainers. With the memory budget, 32 GB machines fit it only with sequential model loading. See [models.md](models.md).
- **Diarization uses NVIDIA Nemotron 3 Diarization** run by a pinned, source-built NeMo-Speech.cpp sidecar, instead of sherpa-onnx's pyannote pipeline. Speaker *recognition* (voice profiles) still uses TitaNet-small embeddings via sherpa-onnx. See [models.md](models.md).
- **Parakeet is offline-only in sherpa-onnx,** so "streaming" is VAD segmentation + offline decoding of each bounded chunk. This is the approach upstream uses too.
