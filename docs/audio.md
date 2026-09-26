# Audio capture

Implementation: `src-tauri/src/audio/` (CPAL 0.18.2) and `meetings/recorder.rs`.

## Two separate streams

| Stream | macOS | Windows |
|---|---|---|
| Microphone | CoreAudio input device (selected or default) | WASAPI capture endpoint |
| Meeting audio | CoreAudio **process tap** on the chosen output device (CPAL builds a `CATapDescription` + private aggregate device). Needs macOS 14.6+. | WASAPI **loopback** on the chosen render endpoint (`AUDCLNT_STREAMFLAGS_LOOPBACK`). No virtual cable. |

The two streams are never pre-mixed:
- Each is written to its own WAV, in its original sample rate and channel layout (16-bit PCM).
- Each is separately resampled to 16 kHz mono f32 for speech.
- A device reconnect starts a new WAV segment (`system-1.wav`, …) aligned to the meeting timeline by `start_offset_ms`.

## Pipeline

```
CPAL callback ─► SPSC ring buffer (4 s) ─► capture worker ─┬─► WAV (header rewritten every 2 s)
 (convert to f32, push; no locks,                          ├─► level meter (≈12/s)
  no I/O, no allocation after warm-up)                     └─► downmix → rubato FFT resampler → 16 kHz
                                                                 → 0.5 s blocks → try_send → live transcriber
```

- **Sample formats.** F32, F64, I8/16/24/32 and U8/16 are all converted. The default config may be an integer format, so nothing assumes F32.
- **Back-pressure.** A full ring buffer drops samples, counts them and shows a warning. A full speech queue skips live transcription for that block. The skipped audio is still on disk, and the post-meeting pass re-transcribes the gap. Capture is never blocked by disk, inference, the UI or integrations.
- **Chunking.** Silero VAD produces speech spans (max 45 s). Spans longer than the target (20 s by default, configurable up to 30 s) are cut at the quietest 30 ms window between 75% of the target and the maximum. Cuts never overlap, so no word is transcribed twice.

## Permissions and warnings

- **macOS microphone.** The status comes from `AVCaptureDevice authorizationStatusForMediaType`. The request is made from onboarding.
- **macOS "System Audio Recording"** (`NSAudioCaptureUsageDescription`). There is no public API to query it, and a denied tap delivers silent buffers rather than an error. So:
  - onboarding explains the permission and links to System Settings;
  - during a recording, if the microphone hears speech for 5 s or more but meeting audio has been exact digital silence for 30 s, a warning names the permission and the output-device mismatch as likely causes.
- **Windows** has no loopback permission. Microphone access is governed by Settings → Privacy → Microphone, and onboarding links there.
- **If meeting audio can't start or disconnects,** microphone recording continues and a prominent warning with **Reconnect** is shown. The app never silently records only the microphone when meeting audio was expected.

## Crash safety

- Meeting state and track progress are persisted every few seconds.
- After a crash, `recording` meetings are marked `interrupted`. On recovery, the WAV headers are repaired from the file length (`audio/wav.rs::repair_header`) and the meeting is processed normally.

## Verified

On the development Mac (macOS 26, M4 Max), a real microphone and system-tap recording round trip passed (`real_device_recording_round_trip`). Windows capture has not yet been run on hardware; see [status.md](status.md).
