# Capability detection

Code: `src-tauri/src/system/`.

## What is detected, and how

No shell output is parsed.

| Fact | macOS | Windows |
|---|---|---|
| OS, version, architecture | `sysinfo` | `sysinfo` |
| CPU model, physical/logical cores | `sysinfo`, `sysctl machdep.cpu.brand_string` | `sysinfo` |
| AVX2/AVX-512 | n/a (ARM) | `is_x86_feature_detected!` |
| Apple Silicon generation/variant | parsed from the brand ("Apple M2 Pro") | n/a |
| Total/available memory | `sysinfo` | `sysinfo` |
| Memory pressure | `sysctl kern.memorystatus_vm_pressure_level` | `GlobalMemoryStatusEx` load % |
| Disk free (volume holding app data) | `sysinfo` disks, longest mount-point match | same |
| GPU(s), VRAM | Metal `MTLCreateSystemDefaultDevice`: name, unified memory, `recommendedMaxWorkingSetSize` | DXGI adapters: name, vendor, `DedicatedVideoMemory` (software adapter skipped) |
| Metal / CUDA / Vulkan | Metal device | `nvcuda.dll` loads / Vulkan loader + instance version |
| Microphone devices, permission | CPAL + `AVCaptureDevice` authorization | CPAL (no queryable permission) |
| System audio availability | macOS ≥ 14.6 and an output device | an output device (WASAPI loopback) |
| Models installed | model manager (files + recorded SHA-verified install) | same |

- **First launch** (onboarding "Check my computer") runs full detection plus a sub-second synthetic benchmark measuring memory bandwidth and FMA throughput. It downloads nothing.
- **Later launches** reuse the cached static facts and refresh memory, pressure, disk and audio devices.

## Memory budget (`memory_budget.rs`)

```
planning budget = total − OS reserve (20% of RAM, clamped 2–6 GiB)
                        − app overhead (600 MiB)
                        − meeting-app allowance (1.5 GiB for Teams/Zoom/Meet)
live budget     = (available − 1 GiB floor) × pressure factor (1.0 / 0.85 / 0.6), ≤ planning
```

Model fit:
- **Required memory** = (weights + KV bytes/token × context + fixed state and compute) × 1.10.
- **Discrete VRAM** (minus a 768 MiB reserve) takes part of the load, split proportionally like llama.cpp's layer offload.
- **Unified memory** is limited by Metal's working set, which only affects the offload fraction.
- **Fit modes:**
  - *Concurrent:* the LLM fits alongside the speech working set.
  - *Sequential:* it fits only after the speech models are unloaded.
  - *Does not fit.*

## Tiers (`profile.rs`)

The recommended tier is `min(memory ceiling, evidence cap)`.

**Memory ceiling** (from the planning budget):
- **Full:** the 30B fits at a 16k or 32k context (sequential allowed).
- **Enhanced:** the 4B fits concurrently at 32k with ≥ 8 GiB extra headroom.
- **Standard:** the 4B fits concurrently at 16k and the machine meets the published minimum.
- **Basic:** otherwise, if the speech stack fits.

**Evidence cap:**
- *Before benchmarks,* the acceleration cap applies:
  - Metal, or a CUDA/Vulkan GPU with ≥ 6 GiB, allows Full;
  - CPU-only allows Enhanced;
  - no AVX2 means Basic;
  - fewer than 4 cores means Standard.
- *Benchmarks* can then promote or demote the tier. See [system-requirements.md](system-requirements.md).

**Before loading the LLM,** `plan_for_live_conditions` re-checks the live budget:
- it may pick a smaller context or the 4B model;
- it unloads the speech models if loading must be sequential;
- it refuses with a clear message instead of forcing the machine into swap.

**Fixture results:**

| Fixture | Tier | Model |
|---|---|---|
| 8 GB Intel Windows laptop | Basic | 4B (sequential, 8k context) |
| 16 GB M1 Mac | Standard | 4B |
| 16 GB CPU-only Windows | Standard | 4B |
| 16 GB Windows + 8 GB RTX | Enhanced | 4B, 32k context |
| 24 GB Apple Silicon | Enhanced | 4B |
| 32 GB Apple Silicon | Full | 30B (sequential) |
| 32 GB Windows + 12 GB RTX | Full | 30B |
| 32 GB CPU-only Windows | Enhanced → Full | Full after a fast 4B benchmark |
| 48 GB M4 Max | Full | 30B |

Fixtures live in `src-tauri/fixtures/hardware/*.json`; tests are in `profile.rs` and `requirements.rs`. In debug builds, Settings → Advanced can simulate any fixture, and a banner is shown while it does.

## User-facing result

There is no numeric score. The result screen shows:
- one of "Your computer is ready", "Your computer is compatible", "Limited performance" or "This computer can't run Minutes reliably";
- Excellent / Good / Standard / Limited ratings for transcription, speaker recognition and summaries, marked as estimated until the model benchmarks have run;
- the recommended model and its download size.
