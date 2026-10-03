# Publishing a release

CI builds and tests Apple Silicon macOS and Windows x64 on pushes to `main`, pull requests, version tags, and manual dispatches. Both platforms must succeed before the tag's publish job runs.

1. Update the version in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and `src-tauri/tauri.conf.json`.
2. Add `docs/releases/vVERSION.md` and update the website's patch notes.
3. Push the changes and a matching `vVERSION` tag. The workflow produces `minutes-macos-arm64` and `minutes-win-x64` artifacts, each containing an installer with a stable filename and a SHA-256 checksum.
4. Publish both installers together to the public `waltzaround/minutes-releases` repository. The source repository is private.
5. Deploy the website after the public release assets are available: `npx wrangler@4 deploy --config website/wrangler.jsonc`.

For automatic tag publishing, configure the source repository's `RELEASE_TOKEN` secret with Contents write access to **only** the public releases repository. Without that secret the workflow keeps the downloads as artifacts and reports that publishing is manual. Do not put a developer's GitHub login token into source control.

Manual publishing from an authenticated GitHub CLI:

```powershell
gh run download RUN_ID --repo waltzaround/minutes --dir dist-release
gh release create vVERSION --repo waltzaround/minutes-releases --title "Minutes VERSION (preview)" --notes-file docs/releases/vVERSION.md --draft
Get-ChildItem dist-release -Recurse -File | ForEach-Object {
  gh release upload vVERSION $_.FullName --repo waltzaround/minutes-releases
}
gh release edit vVERSION --repo waltzaround/minutes-releases --draft=false --latest
```

Keep the release draft until both platforms and checksums have uploaded. The website uses `/releases/latest/download/Minutes-macOS-arm64.dmg` and `/releases/latest/download/Minutes-Windows-x64-setup.exe`.
