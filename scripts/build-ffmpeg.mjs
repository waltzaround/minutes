// Build-time only: compile a standalone LGPL decoder for each installer.
// No external codec libraries, runtime downloads, or system FFmpeg are used.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { availableParallelism } from "node:os";
import { join } from "node:path";

const VERSION = "8.0.1";
const SHA256 = "05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41";
const hostTarget = process.platform === "darwin" && process.arch === "arm64" ? "macos-arm64"
  : process.platform === "win32" && process.arch === "x64" ? "win-x64" : undefined;
const target = process.argv[2] ?? hostTarget;
if (!hostTarget || target !== hostTarget) throw new Error(`Build FFmpeg on its target host (macos-arm64 or win-x64); got ${target}`);
const isWin = target === "win-x64";
const binary = isWin ? "ffmpeg.exe" : "ffmpeg";
const root = join(import.meta.dirname, "..", "src-tauri", "binaries");
const cache = join(root, ".cache", "ffmpeg");
const archive = join(cache, `ffmpeg-${VERSION}.tar.xz`);
const src = join(cache, `ffmpeg-${VERSION}`);
const build = join(cache, `build-${VERSION}-${target}`);
const out = join(root, "ffmpeg");
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const options = [
  "--disable-autodetect", "--disable-gpl", "--disable-nonfree", "--disable-version3",
  "--disable-shared", "--enable-static", "--disable-debug", "--enable-small", "--disable-doc",
  "--disable-x86asm", "--disable-everything", "--enable-ffmpeg", "--disable-ffplay", "--disable-ffprobe",
  "--disable-network", "--enable-protocol=file,pipe", "--enable-demuxers", "--enable-parsers",
  "--enable-decoder=aac,aac_fixed,ac3,ac3_fixed,eac3,alac,flac,opus,vorbis,mp1*,mp2*,mp3*,pcm_*,adpcm_*,amrnb,amrwb,wmav1,wmav2,wmapro,wmavoice,ape,wavpack,wrapped_avframe,rawvideo",
  "--enable-encoder=pcm_s16le,aac,mpeg4", "--enable-muxer=wav,mov,mp4",
  "--enable-indev=lavfi", "--enable-filter=aresample,aformat,anull,anullsrc,sine,color,format,null",
  ...(isWin ? ["--target-os=mingw32", "--arch=x86_64", "--cc=gcc", "--extra-ldflags=-static"]
    : ["--target-os=darwin", "--arch=arm64", "--cc=/usr/bin/clang"]),
];
mkdirSync(cache, { recursive: true });
if (!existsSync(archive) || hash(readFileSync(archive)) !== SHA256) {
  console.log(`Downloading official FFmpeg ${VERSION} source`);
  const response = await fetch(`https://ffmpeg.org/releases/ffmpeg-${VERSION}.tar.xz`);
  if (!response.ok) throw new Error(`Download failed: ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (hash(bytes) !== SHA256) throw new Error("FFmpeg source checksum mismatch");
  writeFileSync(archive, bytes);
}
// Invalidate source and compilation caches whenever this recipe changes.
const recipe = hash(readFileSync(new URL(import.meta.url)));
const stamp = join(build, "minutes-recipe");
if (!existsSync(stamp) || readFileSync(stamp, "utf8") !== recipe || !existsSync(join(build, binary))) {
  rmSync(src, { recursive: true, force: true });
  rmSync(build, { recursive: true, force: true });
  // Use a relative archive name: GNU tar treats Windows drive letters
  // as remote-host prefixes when given an absolute archive path.
  execFileSync("tar", ["-xf", `ffmpeg-${VERSION}.tar.xz`], { cwd: cache, stdio: "inherit" });
  mkdirSync(build, { recursive: true });
  const shellPath = (path) => isWin ? path.replaceAll("\\", "/").replace(/^([A-Za-z]):/, (_, drive) => `/${drive.toLowerCase()}`) : path;
  const quote = (value) => `'${value.replaceAll("'", "'\\''")}'`;
  const configure = [shellPath(join(src, "configure")), ...options].map(quote).join(" ");
  const environment = isWin ? "export PATH=/mingw64/bin:/usr/bin:$PATH; "
    : "export PATH=/usr/bin:/bin:/usr/sbin:/sbin:$PATH MACOSX_DEPLOYMENT_TARGET=14.6; ";
  execFileSync("bash", ["--noprofile", "--norc", "-c", `${environment}cd ${quote(shellPath(build))} && ${configure} && make -j${Math.min(availableParallelism(), 8)}`], { stdio: "inherit" });
  writeFileSync(stamp, recipe);
}
// Keep source and the exact recipe with the executable for redistribution.
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
copyFileSync(join(build, binary), join(out, binary));
if (!isWin) chmodSync(join(out, binary), 0o755);
copyFileSync(join(src, "COPYING.LGPLv2.1"), join(out, "LICENSE"));
copyFileSync(join(src, "LICENSE.md"), join(out, "LICENSE.upstream"));
copyFileSync(join(src, "README.md"), join(out, "README.upstream"));
copyFileSync(archive, join(out, `ffmpeg-${VERSION}-source.tar.xz`));
copyFileSync(new URL(import.meta.url), join(out, "build-ffmpeg.mjs"));
writeFileSync(join(out, "VERSION"), `${VERSION} ${target}\n`);
writeFileSync(join(out, "NOTICE"), `Minutes includes FFmpeg ${VERSION} as a separate executable for local media import.
FFmpeg is licensed under LGPL 2.1 or later. See LICENSE and LICENSE.upstream.
Source: https://ffmpeg.org/releases/ffmpeg-${VERSION}.tar.xz
Source SHA-256: ${SHA256}
The corresponding source archive and build script are included in this folder.
Configure options: ${options.join(" ")}
`);
console.log(`Installed FFmpeg ${VERSION} (${target}) into ${out}`);
