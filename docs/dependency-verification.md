# Dependency verification log

Each major dependency was checked against its current source/docs before use
(spec §34). Dates: 2026-09-26. Toolchain: rustc 1.98 (sysinfo 0.39 requires 1.95).

## CPAL 0.18.2 (audio)
- `Device::name()` was removed. Use `device.description()?.name()` or `Display`.
- `device.id()? -> DeviceId` is stable across restarts, `Display` + `FromStr`, so it is safe to persist.
- `build_input_stream::<T,_,_>(StreamConfig, data_cb, err_cb, Option<Duration>)`. **Streams do not
  auto-start since 0.18**; you must call `play()`.
- Default configs may be I24/I32, so always match on `SampleFormat`.
- `cpal::Stream` is `Send + Sync` (compile-checked on macOS).
- **Loopback:** build an *input* stream on an *output* device, using the output device's default config.
  - macOS: CoreAudio process tap (`CATapDescription` + private aggregate device). Requires
    macOS 14.6+ and the `NSAudioCaptureUsageDescription` Info.plist key.
    - There is no public API to query or request this permission. When it is denied, the tap delivers
      all-zero buffers, so we detect silence heuristically.
    - cpal only uses the tap when the output device has no input side.
  - Windows: WASAPI `AUDCLNT_STREAMFLAGS_LOOPBACK` on any render endpoint.
- No feature flags are required.

## sysinfo 0.39.6
- `System::physical_core_count()` is an associated function.
- `long_os_version()` is for display. `name()` returns "Darwin" on macOS.
- `Disks::new_with_refreshed_list()`. `/` and `/System/Volumes/Data` both appear, so dedupe.

## macOS native
- Microphone permission: `AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeAudio)` and
  `requestAccessForMediaType_completionHandler` (objc2-av-foundation 0.3, features
  `AVCaptureDevice AVMediaFormat block2`).
- Metal: `MTLCreateSystemDefaultDevice` (must link CoreGraphics), `recommendedMaxWorkingSetSize`,
  `hasUnifiedMemory`.
- `sysctlbyname("kern.memorystatus_vm_pressure_level")`: 1 = normal, 2 = warn, 4 = critical.

## Windows native
- GPUs: DXGI `CreateDXGIFactory1` / `EnumAdapters1` / `GetDesc1` (`DedicatedVideoMemory`). Skip
  `DXGI_ADAPTER_FLAG_SOFTWARE`.
- CUDA driver present if `nvcuda.dll` loads. AVX2 via `is_x86_feature_detected!`.
- Vulkan: `ash::Entry::load()` + `try_enumerate_instance_version()`.

## keyring 4.2.0
- Default features select Keychain / Credential Manager per OS. `Entry::new(service, user)`.
- `Error::NoEntry` on a missing credential. Round trip verified on macOS.

## Notion API
- `Notion-Version: 2026-03-11` (latest).
- Pages are created under `parent: {type: "data_source_id", data_source_id}`.
  - `GET /v1/databases/{id}` returns `data_sources[]`.
  - Search filter value is `"data_source"`.
  - Title property comes from `GET /v1/data_sources/{id}`.
- Limits:
  - 2000 chars per text object.
  - 100 children per request; append the rest with `PATCH /v1/blocks/{id}/children` and
    `position: {type: "end"}`.
  - Rate limit 180 req/min. On 429 or 529, honour `Retry-After` (seconds).

## Linear GraphQL
- `POST https://api.linear.app/graphql`.
- Personal API key header: `Authorization: <key>` (no Bearer). OAuth uses Bearer.
- `issueCreate(input: IssueCreateInput)` → `{ success issue { id identifier url } }`.
  - `IssueCreateInput.id` accepts a client-generated UUID v4. We generate it once per action item and
    reuse it on every retry, so retries cannot create duplicates. If the create "fails" after an
    unknown outcome, look it up with `issue(id:)`.
  - Priority: 0 = none, 1 = urgent, 2 = high, 3 = medium, 4 = low. `dueDate` is `"YYYY-MM-DD"`.
- Rate limiting returns HTTP 400 with `errors[].extensions.code == "RATELIMITED"`.

## sherpa-onnx 1.13.8 (official crate by the sherpa-onnx author)
- We use it rather than `sherpa-rs`, which is archived and has no timestamps.
- Linking: prebuilt **static** libraries (onnxruntime included) are downloaded by `sherpa-onnx-sys/build.rs`.
  - No dylib or dll needs to be bundled.
  - For offline or pinned CI builds, set `SHERPA_ONNX_ARCHIVE_DIR` / `SHERPA_ONNX_LIB_DIR`.
- Parakeet TDT v3 is **offline-only**. Upstream's streaming approach is VAD + offline decode per segment, which is
  what we do.
  - `OfflineRecognizerResult.timestamps` are per token, in seconds relative to the segment.
- Silero VAD: `SileroVadModelConfig` defaults are all zero, so every field must be set. `max_speech_duration`
  bounds segment length.
- `OfflineSpeakerDiarization::process(&[f32])` is a single blocking call.
  - The C API has a progress callback that the safe wrapper does not bind.
  - The clustering threshold is very sensitive (0.5 → 1 speaker and 0.3 → 4 speakers on the same 2-speaker
    sample), so it must be calibrated.
- `SpeakerEmbeddingExtractor` + `SpeakerEmbeddingManager` handle enrollment and matching. The manager is
  in-memory, so we persist the embeddings ourselves (encrypted).

## llama.cpp (b11193, 2026-09-26)
- Binaries come from the `bNNNNN` pre-release assets. `llama-server` is now a thin launcher, so all dylibs/dlls
  in the archive must be shipped next to it.
  - macOS arm64 builds have Metal embedded.
  - Windows ships cpu / vulkan / cuda-12.4 / cuda-13.4 variants.
- Flags we rely on:
  - `--host 127.0.0.1 --port <ephemeral> -m -c <ctx> -ngl <n|auto> -t -np 1 --no-webui`
  - `--reasoning off` and `--api-key <random>` (`/health` is exempt from the key).
  - `-c` must always be set, because the default is the model's own 1M-token context.
- `/health` returns 503 while the model is loading and 200 `{"status":"ok"}` once ready.
- Structured output: `response_format: {"type":"json_schema","json_schema":{"schema":{...}}}`. The README's
  unwrapped form is ignored by the server.
- Non-streaming chat completions return `timings.{prompt_per_second,predicted_per_second}`.
- Nemotron 3 thinks by default. Disable it with `chat_template_kwargs: {"enable_thinking": false}` (a real boolean).

## Nemotron 3 Nano GGUFs
- 4B: official `nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF`, Q4_K_M, 2.84 GB.
  - Measured KV cache 16 KiB/token, 81 MiB recurrent state.
- 30B-A3B: there is no official NVIDIA GGUF. We use `ggml-org/NVIDIA-Nemotron-3-Nano-30B-A3B-GGUF` Q4_K_M
  (llama.cpp maintainers), **22.4 GB** (the spec estimated ~18 GB).
  - KV cache ≈ 6 KiB/token.
  - At this size 32 GB machines fit it only with sequential model loading.
- License: NVIDIA Nemotron Open Model License.
- HF `resolve` URLs support HTTP Range (206). Re-request the `resolve` URL on every resume because the signed CDN
  URLs expire. Verify the sha256 against `lfs.oid`.

## Nemotron 3 Diarization + NeMo-Speech.cpp (2026-09-26)
- `nvidia/Nemotron-3-Diarization`:
  - Streaming Sortformer with an Arrival-Order Speaker Cache, up to 8 speakers, 16 kHz input.
  - Published as `.nemo`, safetensors (transformers) and `q8_0` GGUF (107,012,128 bytes, sha256
    `08456d9e…c7a3a1`).
  - License: OpenMDW-1.1.
  - It produces speaker turns but no voice embeddings, so enrolled-speaker matching keeps TitaNet.
- NeMo-Speech.cpp (NVIDIA, Apache-2.0) is the model card's Python-free runtime.
  - Release v0.1.0 (2026-08-19) fails on this model with `sortformer: pre_ln transformer variant is not
    supported`.
  - Support landed in #50 and #52 on `main`. We build commit `97a15af` (`metal-diar` preset on macOS,
    `cpu-diar` on Windows).
- CLI contract used:
  - `nemo-speech diarize <wav> --model <gguf> --format json --device auto|cpu --output <file>`.
  - Output is `{"segments":[{"start","end","speaker"}]}` with 1-based speakers.
- macOS build notes:
  - Upstream links SentencePiece statically only on Linux, so we pass `SENTENCEPIECE_STATIC_LIB`.
  - We guard the GNU-only `--exclude-libs` flag behind `NOT APPLE`.
  - We set `CMAKE_POLICY_VERSION_MINIMUM=3.5` for CMake 4.
  - We rewrite rpaths to `@loader_path`.
  - The script fails if any Homebrew library is linked.
