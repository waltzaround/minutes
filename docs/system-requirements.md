# System requirements

## Officially supported minimum

**macOS**
- macOS 14.6 or newer. This is required by the system-audio capture mechanism.
- Apple Silicon (M1 or newer). Intel Macs are not a release target.
- 16 GB unified memory.
- 10 GB free disk space.
- A working microphone, plus "System Audio Recording" permission for meeting audio.

**Windows**
- Windows 10 64-bit or newer on x86-64 Intel or AMD.
- Practical target: Core i5 8th gen or Ryzen 5 3000 series or newer, 4+ physical cores, AVX2.
- 16 GB RAM.
- 10 GB free disk space.
- A working microphone. Meeting audio uses WASAPI loopback, with no virtual cable.

**The 16 GB minimum is the official supported experience (the Standard tier):**
- live transcript;
- diarization and known-speaker recognition;
- Nemotron 3 Nano 4B summaries and action extraction;
- Notion and Linear.

## Recommended

- **macOS:** M2 Pro or newer, 24–32 GB+, 30 GB free.
- **Windows:** 32 GB RAM, a modern discrete GPU where available, 30 GB free.

## Full local AI (Nemotron 3 Nano 30B-A3B)

This typically needs 32 GB+ unified or system memory on an Apple Silicon Pro/Max, or a Windows system with enough RAM and/or GPU VRAM.

The 30B Q4 model is a **~22.4 GB download**, so budget **30 GB free disk space**. On a 32 GB Mac it fits only with sequential loading: the speech models are unloaded before it loads, which the app does automatically. When free memory is short at analysis time, the app falls back to a smaller context or the 4B model and tells you.

## 8 GB: limited mode (usable, not supported)

8 GB machines continue in **Basic** mode with a clear warning:

> Your computer has 8 GB of memory. Recording and transcription may work, but local AI summaries can be slower and other applications may affect reliability. 16 GB or more is recommended.

In Basic mode:
- **Works:** recording, Parakeet transcription, diarization/recognition where memory allows, transcript editing, Notion and Linear.
- **Limited:** summaries use the 4B model with an 8k or 4k context, and models load one at a time.
- **Not available:** the 30B model, and running speech and LLM models simultaneously.

The app blocks only when the transcription benchmark shows the machine cannot keep up at all (RTF above 4).

## Storage

| Item | Size |
|---|---|
| App + runtime | ~0.1 GB (plus WebView) |
| Parakeet ASR | 0.67 GB |
| VAD + diarization + embeddings | < 0.05 GB |
| Nemotron 4B Q4 | 2.8 GB |
| Nemotron 30B-A3B Q4 | 22.4 GB |
| Working reserve kept free | 3 GB + 5% of any download |
| Meeting audio | about 690 MB per hour of stereo 48 kHz (16-bit) per stream while retained. By default it is deleted once processing succeeds. |

The downloader refuses to start if a download plus the working reserve would not fit, so it never fills the disk.

## Why a GPU is optional

- **Speech** (Parakeet, VAD, diarization, embeddings) runs on the CPU through ONNX Runtime. It is comfortably faster than real time on any supported machine: measured RTF 0.066 on an M4 Max.
- **llama.cpp** uses Metal on Apple Silicon and Vulkan on Windows GPUs when available, and CPU inference is always the fallback.
- Token generation speed is dominated by memory bandwidth and active parameters. The 30B-A3B model activates only ~3.5B parameters per token, so fast CPU-only desktops can still qualify for Full after a benchmark.

## How benchmarks change recommendations

Hardware detection gives a provisional tier. Benchmarks can move it up or down:

- **Transcription RTF**
  - above 0.35: capped at Standard;
  - above 0.6: capped at Basic, and the live transcript is turned off (the transcript is produced after the meeting);
  - above 4: speech is not viable.
- **4B generation speed**
  - ≥ 20 tokens/s: Full is possible if memory permits and the estimated 30B speed is ≥ 10 tokens/s;
  - ≥ 10 tokens/s: Enhanced;
  - ≥ 5 tokens/s: Standard;
  - otherwise Basic.
  - An unstable server or invalid structured output disables summaries on that model.
- **30B benchmark:** below 10 tokens/s generation or 80 tokens/s prompt processing demotes Full to Enhanced.

**Examples:**
- A 32 GB CPU-only desktop starts at Enhanced and is promoted to Full after a fast 4B benchmark.
- A 32 GB Mac with a slow 30B benchmark is demoted to Enhanced with the 4B model.
