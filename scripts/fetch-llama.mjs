// Downloads the pinned llama.cpp release for a target platform, verifies its
// SHA-256, and installs the llama-server runtime into
// src-tauri/binaries/llama/ (bundled as an app resource).
//
//   node scripts/fetch-llama.mjs            # current platform
//   node scripts/fetch-llama.mjs win-x64    # cross-prepare Windows (CI)
//
// Windows ships the Vulkan build: it runs on NVIDIA, AMD and Intel GPUs and
// falls back to the bundled CPU backends when no Vulkan device exists.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { cpSync, createReadStream, existsSync, mkdirSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

const RELEASE = "b11193";
const ASSETS = {
  "macos-arm64": {
    name: `llama-${RELEASE}-bin-macos-arm64.tar.gz`,
    sha256: "d95b9af91fcf519db2c9fba9068ba75d4e858df35c8e6a08bec3e10055bb4fe2",
    // Files the server needs, copied with symlinks resolved.
    files: (dir) => [
      "llama-server",
      "libllama-server-impl.dylib",
      ...readdirSync(dir).filter((f) => /^lib[\w-]+\.0\.dylib$/.test(f)),
      "LICENSE",
    ],
  },
  "win-x64": {
    name: `llama-${RELEASE}-bin-win-vulkan-x64.zip`,
    sha256: "a106346b30d79883626bd9cf9f0d787ff949eeaf3feaf6a4f31d41a2ecf9086b",
    files: (dir) => ["llama-server.exe", ...readdirSync(dir).filter((f) => f.endsWith(".dll")), ...readdirSync(dir).filter((f) => /^LICENSE/.test(f))],
  },
};

const target = process.argv[2] ?? (process.platform === "win32" ? "win-x64" : "macos-arm64");
const asset = ASSETS[target];
if (!asset) throw new Error(`unknown target ${target}`);

const root = join(import.meta.dirname, "..", "src-tauri", "binaries");
const cache = join(root, ".cache");
const out = join(root, "llama");
mkdirSync(cache, { recursive: true });
const archive = join(cache, asset.name);

async function sha256(path) {
  const h = createHash("sha256");
  for await (const chunk of createReadStream(path)) h.update(chunk);
  return h.digest("hex");
}

if (!existsSync(archive) || (await sha256(archive)) !== asset.sha256) {
  const url = `https://github.com/ggml-org/llama.cpp/releases/download/${RELEASE}/${asset.name}`;
  console.log(`downloading ${url}`);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`download failed: ${res.status}`);
  await writeFile(archive, Buffer.from(await res.arrayBuffer()));
}
const digest = await sha256(archive);
if (digest !== asset.sha256) throw new Error(`checksum mismatch for ${asset.name}: ${digest}`);

const extract = join(cache, `${target}-extract`);
rmSync(extract, { recursive: true, force: true });
mkdirSync(extract, { recursive: true });
execFileSync("tar", ["-xf", archive, "-C", extract]);
// Archives contain a single top-level folder (macOS) or files at the root (Windows).
const entries = readdirSync(extract);
const src = entries.length === 1 && statSync(join(extract, entries[0])).isDirectory() ? join(extract, entries[0]) : extract;

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
for (const f of asset.files(src)) {
  const from = join(src, f);
  if (!existsSync(from)) throw new Error(`missing ${f} in archive`);
  cpSync(realpathSync(from), join(out, f), { dereference: true });
}
writeFileSync(join(out, "VERSION"), `${RELEASE} ${target}\n`);
console.log(`installed llama.cpp ${RELEASE} (${target}) into ${out}`);
