# Packaging for macOS

## Build

```bash
node scripts/fetch-llama.mjs
```

```bash
pnpm tauri build
```

This produces `src-tauri/target/release/bundle/macos/Minutes.app` and `…/dmg/Minutes_0.1.0_aarch64.dmg`.

- **Target:** Apple Silicon, minimum macOS 14.6 (`bundle.macOS.minimumSystemVersion`). Intel is not a release blocker and is not built.
- **Info.plist** (`src-tauri/Info.plist`) includes `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription`.
- **Entitlements** (`src-tauri/entitlements.plist`): `com.apple.security.device.audio-input`. The hardened runtime is enabled.
- **llama.cpp runtime:** bundled as a resource at `Contents/Resources/llama/`. It includes `llama-server`, `libllama-server-impl.dylib` and the `lib*.0.dylib` libraries, resolved via `@loader_path`.
- **NeMo-Speech.cpp diarization runtime:** bundled at `Contents/Resources/nemo-speech/` (`nemo-speech` + ggml dylibs via `@loader_path`), built by `scripts/build-nemo-speech.mjs`. Sign it along with the llama runtime.
- **sherpa-onnx and ONNX Runtime** are linked statically into the app binary, so there are no extra dylibs.

### DMG step in headless sessions

Tauri's `bundle_dmg.sh` uses Finder AppleScript to lay out the DMG window. In non-interactive shells, CI, or terminals without Automation permission, that step fails with `error running bundle_dmg.sh`, even though `Minutes.app` has already been built. Either grant the terminal *Automation → Finder* permission, or build the DMG without the layout step:

```bash
cd src-tauri/target/release/bundle/dmg
```

```bash
mkdir -p /tmp/minutes-dmg && cp -R ../macos/Minutes.app /tmp/minutes-dmg/
```

```bash
./bundle_dmg.sh --skip-jenkins --volname Minutes --app-drop-link 400 150 --icon Minutes.app 150 150 Minutes_0.1.0_aarch64.dmg /tmp/minutes-dmg
```

The v0.1 DMG (24 MB) was produced this way on the development machine. The release app launched, applied migrations and found the installed models.

## Signing and notarisation

1. Set the Tauri signing environment:
   - `APPLE_SIGNING_IDENTITY="Developer ID Application: …"`
   - `APPLE_ID`, `APPLE_PASSWORD` (app-specific password) and `APPLE_TEAM_ID`, or an App Store Connect API key.
2. **Sign the sidecar before bundling.** Tauri signs the main binary but not resources, so the llama runtime must be signed with the same identity and the hardened runtime:
   ```bash
   for f in src-tauri/binaries/llama/*.dylib src-tauri/binaries/llama/llama-server src-tauri/binaries/nemo-speech/*.dylib src-tauri/binaries/nemo-speech/nemo-speech; do
     codesign --force --timestamp --options runtime --sign "$APPLE_SIGNING_IDENTITY" "$f"
   done
   ```
3. Run `pnpm tauri build`. With the variables set, Tauri signs, notarises and staples the app.
4. Verify:
   ```bash
   spctl -a -vv Minutes.app
   ```
   ```bash
   codesign --verify --deep --strict Minutes.app
   ```

## Updates

The Tauri updater is the intended mechanism (signed update bundles and a JSON endpoint on internal hosting). It is not registered yet, because it needs an update signing key pair and an endpoint URL. To enable it:

1. `pnpm tauri signer generate`.
2. Add `tauri-plugin-updater`.
3. Configure `plugins.updater.pubkey` and `endpoints`.
4. Gate checks on Settings → Privacy → "Check for app updates", which is off by default.
