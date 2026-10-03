// Give public downloads stable filenames, independent of the app version.
import { createHash } from "node:crypto";
import { createReadStream, existsSync, mkdirSync, readdirSync, copyFileSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");
const version = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
if (process.env.GITHUB_REF_TYPE === "tag" && process.env.GITHUB_REF_NAME !== `v${version}`) {
  throw new Error("Release tag must match the app version");
}
const platform = process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : null;
if (!platform) throw new Error("Build releases on Windows or macOS");
const folder = platform === "windows" ? "nsis" : "dmg";
const extension = platform === "windows" ? ".exe" : ".dmg";
const source = join(root, "src-tauri", "target", "release", "bundle", folder);
const files = existsSync(source) ? readdirSync(source).filter(f => f.endsWith(extension) && f.includes(version)) : [];
if (files.length !== 1) throw new Error(`Expected one ${version} installer in ${source}; found ${files.length}`);
const name = platform === "windows" ? "Minutes-Windows-x64-setup.exe" : "Minutes-macOS-arm64.dmg";
const output = join(root, "dist-release");
mkdirSync(output, { recursive: true });
const destination = join(output, name);
copyFileSync(join(source, files[0]), destination);
const hash = createHash("sha256");
for await (const chunk of createReadStream(destination)) hash.update(chunk);
writeFileSync(`${destination}.sha256`, `${hash.digest("hex")}  ${name}\n`);
console.log(`Prepared ${name} for v${version}`);
