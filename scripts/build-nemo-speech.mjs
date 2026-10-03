// Builds NVIDIA NeMo-Speech.cpp (diarization only) from a pinned commit and
// installs the `nemo-speech` runtime into src-tauri/binaries/nemo-speech/
// (bundled as an app resource). It runs Nemotron 3 Diarization.
//
//   node scripts/build-nemo-speech.mjs
//
// Why build from source: Nemotron 3 Diarization support (NeMo-Speech.cpp
// #50/#52) landed after the only published release (v0.1.0), whose binaries
// fail with "pre_ln transformer variant is not supported". Switch this to the
// verified release artifacts once a release includes it.
//
// Prerequisites:
//   macOS:   Xcode Command Line Tools; brew install cmake ninja
//   Windows: Visual Studio 2022 C++ tools, CMake 3.26+, Ninja, and vcpkg SentencePiece
//            (vcpkg install sentencepiece:x64-windows-static; set CMAKE_TOOLCHAIN_FILE)
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const REPO = "https://github.com/NVIDIA/NeMo-Speech.cpp";
const COMMIT = "97a15afa5caa9bce5baaa86c1184103877af4101"; // feat(diar): make Nemotron 3 Diarization the default diarizer (#52)
const MACOS_TARGET = "14.6";

const root = join(import.meta.dirname, "..", "src-tauri", "binaries");
const src = join(root, ".cache", "nemo-speech-src");
const out = join(root, "nemo-speech");
const isMac = process.platform === "darwin";
const isWin = process.platform === "win32";
// Windows uses the CPU build: it needs no Vulkan SDK to build and already
// diarizes several times faster than real time.
const preset = isMac ? "metal-diar" : "cpu-diar";

const env = {
  ...process.env,
  // SentencePiece's pinned CMakeLists predates CMake 4.
  CMAKE_POLICY_VERSION_MINIMUM: "3.5",
  ...(isMac ? { MACOSX_DEPLOYMENT_TARGET: MACOS_TARGET } : {}),
};
const run = (cmd, args, cwd = src) => execFileSync(cmd, args, { cwd, stdio: "inherit", env });

// 1. Pinned checkout.
if (!existsSync(join(src, ".git"))) {
  mkdirSync(src, { recursive: true });
  run("git", ["init", "-q"]);
  run("git", ["remote", "add", "origin", REPO]);
}
run("git", ["fetch", "-q", "--depth", "1", "origin", COMMIT]);
run("git", ["checkout", "-q", "--force", COMMIT]);
run("git", ["submodule", "update", "--init", "--depth", "1", "ggml"]);

// 2. macOS: link SentencePiece statically (the upstream CMake only does this
//    on Linux; its GNU-only --exclude-libs flag breaks Apple's linker).
const extra = [];
if (isWin) {
  // Match the documented SentencePiece triplet and its static C++ runtime.
  extra.push("-DVCPKG_TARGET_TRIPLET=x64-windows-static", "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded", "-DVCPKG_MANIFEST_MODE=OFF", "-DGGML_NATIVE=OFF");
  if (process.env.VCPKG_INSTALLED_DIR) extra.push(`-DVCPKG_INSTALLED_DIR=${process.env.VCPKG_INSTALLED_DIR}`);
}
if (!isWin) {
  const cm = join(src, "src", "asr", "CMakeLists.txt");
  const text = readFileSync(cm, "utf8");
  const flag = `    target_link_options(\n        nemo_speech_asr PRIVATE "LINKER:--exclude-libs,libsentencepiece.a")`;
  if (text.includes(flag)) {
    writeFileSync(cm, text.replace(flag, `    if(NOT APPLE)\n    ${flag.replace(/\n/g, "\n    ")}\n    endif()`));
  }
  // Same pinned commit and options as upstream scripts/build_sentencepiece_static.sh
  // (whose GNU-only `install -D` fails on macOS).
  const SP_COMMIT = "17d7580d6407802f85855d2cc9190634e2c95624";
  const spSrc = join(src, ".deps", "sentencepiece-src");
  const spBuild = join(src, ".deps", "sentencepiece-build");
  if (!existsSync(join(spSrc, ".git"))) {
    mkdirSync(spSrc, { recursive: true });
    run("git", ["init", "-q"], spSrc);
    run("git", ["remote", "add", "origin", "https://github.com/google/sentencepiece.git"], spSrc);
  }
  run("git", ["fetch", "-q", "--depth", "1", "origin", SP_COMMIT], spSrc);
  run("git", ["checkout", "-q", "--force", SP_COMMIT], spSrc);
  run("cmake", ["-G", "Ninja", "-S", spSrc, "-B", spBuild, "-DCMAKE_BUILD_TYPE=Release", "-DSPM_BUILD_TEST=OFF", "-DSPM_ENABLE_SHARED=OFF", "-DSPM_ENABLE_TCMALLOC=OFF"]);
  run("cmake", ["--build", spBuild, "--target", "sentencepiece-static"]);
  const sp = join(src, ".deps", "sentencepiece");
  mkdirSync(join(sp, "lib"), { recursive: true });
  mkdirSync(join(sp, "include"), { recursive: true });
  cpSync(join(spBuild, "src", "libsentencepiece.a"), join(sp, "lib", "libsentencepiece.a"));
  cpSync(join(spSrc, "src", "sentencepiece_processor.h"), join(sp, "include", "sentencepiece_processor.h"));
  extra.push(`-DSENTENCEPIECE_STATIC_LIB=${join(sp, "lib", "libsentencepiece.a")}`, `-DSENTENCEPIECE_INCLUDE_DIR=${join(sp, "include")}`);
}
if (isMac) extra.push(`-DCMAKE_OSX_DEPLOYMENT_TARGET=${MACOS_TARGET}`);

// 3. Build.
rmSync(join(src, "build", preset), { recursive: true, force: true });
run("cmake", ["--preset", preset, ...extra]);
run("cmake", ["--build", "--preset", preset]);

// 4. Install the runtime next to its libraries (resolved via @rpath/@loader_path on macOS).
const bin = join(src, "build", preset, "bin");
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const wanted = readdirSync(bin).filter((f) =>
  isWin ? f === "nemo-speech.exe" || f.endsWith(".dll") : f === "nemo-speech" || /^lib(ggml[\w-]*\.0|nemo_speech_asr)\.dylib$/.test(f),
);
for (const f of wanted) cpSync(realpathSync(join(bin, f)), join(out, f), { dereference: true });
for (const f of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md"]) {
  if (existsSync(join(src, f))) cpSync(join(src, f), join(out, f));
}
if (isMac) {
  // Load libraries from the runtime's own folder, never from the build tree.
  const fixRpaths = (file) => {
    const rpaths = execFileSync("otool", ["-l", file], { encoding: "utf8" })
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.startsWith("path "))
      .map((l) => l.split(" ")[1]);
    for (const p of rpaths) run("install_name_tool", ["-delete_rpath", p, file], out);
    run("install_name_tool", ["-add_rpath", "@loader_path", file], out);
  };
  const exe = join(out, "nemo-speech");
  for (const f of readdirSync(out).filter((f) => f === "nemo-speech" || f.endsWith(".dylib"))) fixRpaths(join(out, f));
  // Re-sign (ad hoc) after editing load commands; release signing happens later.
  for (const f of readdirSync(out).filter((f) => f === "nemo-speech" || f.endsWith(".dylib"))) {
    run("codesign", ["--force", "--sign", "-", join(out, f)], out);
  }
  const deps = execFileSync("otool", ["-L", exe, ...readdirSync(out).filter((f) => f.endsWith(".dylib")).map((f) => join(out, f))], { encoding: "utf8" });
  if (/\/opt\/homebrew|\/usr\/local/.test(deps)) throw new Error("runtime links Homebrew libraries; it would not run on other Macs");
}
writeFileSync(join(out, "VERSION"), `${COMMIT} ${preset}\n`);
console.log(`installed NeMo-Speech.cpp ${COMMIT.slice(0, 7)} (${preset}) into ${out}`);
