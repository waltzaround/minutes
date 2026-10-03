# Packaging for Windows

## Build (on Windows, x86_64)

Prerequisites: Rust stable (MSVC), Node 22, pnpm, Visual Studio 2022 Build Tools (C++), and WebView2 (preinstalled on Windows 11; the installer bootstraps it on Windows 10). Visual Studio 2019 cannot link the prebuilt sherpa-onnx libraries: it reports unresolved `__std_*` symbols.

```powershell
pnpm install
```

```powershell
node scripts/fetch-llama.mjs win-x64
```

Build the diarization runtime from a Visual Studio 2022 Developer PowerShell, with CMake 3.26+, Ninja, and `sentencepiece:x64-windows-static` installed through vcpkg. Set `CMAKE_TOOLCHAIN_FILE` to vcpkg's `scripts/buildsystems/vcpkg.cmake`. If using a separate vcpkg manifest, also set `VCPKG_INSTALLED_DIR` to its `vcpkg_installed` directory.

```powershell
node scripts/build-nemo-speech.mjs
```

```powershell
pnpm tauri build
```

This produces `src-tauri\target\release\bundle\nsis\Minutes_0.1.0_x64-setup.exe`.

- **NSIS installer, per-user** (`installMode: currentUser`). No admin rights are needed to install or run it.
- **Static CRT** (`+crt-static` in `src-tauri/.cargo/config.toml`). This matches sherpa-onnx's `/MT` static libraries and removes the VC++ redistributable dependency.
- **llama.cpp runtime:** the Vulkan build, bundled at `resources\llama\`. It includes `llama-server.exe`, `ggml-vulkan.dll` and CPU backend DLLs for SSE4.2/AVX2/AVX-512/Zen4, selected at runtime. It uses NVIDIA, AMD or Intel GPUs through Vulkan and falls back to the CPU, so no NVIDIA-specific build is required.
  - A CUDA build exists upstream but is about 650 MB with cudart. It could be offered later as an optional download.
- **Diarization runtime:** NeMo-Speech.cpp CPU build from `scripts/build-nemo-speech.mjs` (needs `vcpkg install sentencepiece:x64-windows-static` and `CMAKE_TOOLCHAIN_FILE`), bundled at `resources\nemo-speech\`.
- **llama-server** is started with `CREATE_NO_WINDOW`, so no console window appears.

## Signing

Set `bundle.windows.certificateThumbprint` (or use `signCommand` for an HSM or Azure Trusted Signing). Sign `llama-server.exe` and the bundled DLLs before building, then Tauri signs the installer and main executable.

## Verification status

The Windows code paths are:
- WASAPI loopback via CPAL;
- DXGI GPU and VRAM detection;
- `GlobalMemoryStatusEx` memory pressure;
- `nvcuda.dll` and Vulkan detection;
- Credential Manager via `keyring`.

They have been written against the verified crate APIs but **have not yet been compiled or run on Windows hardware**. The CI workflow (`.github/workflows/ci.yml`) builds and tests on `windows-latest`. Test on a real machine before distributing; see [status.md](status.md).
