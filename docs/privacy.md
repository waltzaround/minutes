# Privacy and security

## Defaults

- **No telemetry,** analytics SDK or third-party tracking.
- **No remote AI inference and no audio upload.**
- **Network requests happen only when you:**
  - download a model;
  - connect or use Notion or Linear;
  - enable update checks (off by default; the updater is not active until signing keys and an update endpoint are configured).
- **Speaker embeddings stay on this computer.**
  - They are encrypted with AES-256-GCM.
  - The key is kept in the macOS Keychain or Windows Credential Manager.
- **Unknown guests are never enrolled.** Voice profiles are only created by explicit enrollment, or on request after you confirm a person. "Possible" matches are never used as names until confirmed.
- **Recording is always visible:**
  - a pulsing red indicator and timer on the recording screen;
  - "Recording" pinned in the sidebar on every screen;
  - the window title reads "● Recording — Minutes".
- **Consent reminder** (on by default): "Remember to let everyone know the meeting is being recorded."

## Audio retention

Settings → Privacy offers three options:
- **Delete after the transcript is finalised** (default);
- keep for 7 days;
- keep indefinitely.

Audio is only deleted after processing has completed successfully. A failed or interrupted meeting keeps its audio so it can be processed again.

## Secrets

- The Notion token, Linear API key and voiceprint key are stored only in the OS credential store (`keyring`). They are never written to SQLite, settings or logs.
- Diagnostics reports exclude transcripts, audio, voiceprints and credentials.

## Local sidecar

`llama-server` binds to `127.0.0.1` only, on an ephemeral port, and requires a random per-launch API key. It is not reachable from the LAN or from other local processes without the key. It is started only when needed and stopped when idle.

## Voice data controls

Settings → People & Voices and the People screen let you:
- delete one person's voice profile;
- re-record it;
- export the people directory without any voice data;
- clear all voice data, which also deletes the encryption key.
