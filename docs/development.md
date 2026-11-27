# Development

How to run Siphon from source, check it, build an installer and make a release. How the app is put together is in [how-it-works.md](how-it-works.md), and why it works the way it does in [decisions.md](decisions.md).

## Requirements

- Windows 11
- [Node.js](https://nodejs.org) 20 or newer
- [Rust](https://rustup.rs) (stable)
- Microsoft C++ Build Tools ("Desktop development with C++") and WebView2, which ships with Windows 11

You don't need to install yt-dlp, FFmpeg or Deno: Siphon downloads them on first launch.

## Run it

```sh
npm install
npm run tauri dev
```

The first launch shows a setup screen while Siphon downloads its engines (about 150 MB).

The dev build keeps its own settings, sign-ins, downloads and tools, in folders ending in `.dev` (`%APPDATA%\app.siphon.desktop.dev`), so it never touches an installed Siphon's and can run beside it.

To drive the running app from scripts (DevTools protocol, calling backend commands directly), see the [testing notes](decisions.md#testing-notes).

## Checks

```sh
npm run typecheck                                  # TypeScript
npm run lint                                       # oxlint: correctness, React hooks, unused code
npm run knip                                       # unused files, exports and dependencies
cargo test --manifest-path src-tauri/Cargo.toml    # Rust unit tests
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

GitHub runs all of them (`.github/workflows/ci.yml`) on every push to `main` and every pull request, and the release workflow runs them first, so a release is never built from a commit that fails them.

## Build an installer

```sh
npm run tauri build
```

The installer is written to `src-tauri/target/release/bundle/nsis/`.

The build also signs the installer for the self-update, so it needs the signing key. Without it the build stops at the end with an error about a missing private key. In PowerShell:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\siphon.key"   # the key file, or its contents
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""                             # the key has no password
npm run tauri build
```

Released versions are not built by hand: see [Releases](#releases).

## Releases

Pushing a version tag (`v0.5.0`) runs `.github/workflows/release.yml`. It builds and signs the app, and attaches the installer, its signature and `latest.json` (the file installed copies read to find updates) to a **draft** GitHub release. Nothing reaches installed copies until the draft is published on GitHub.

To make a release: set the same version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, then tag and push.

The signing key is in the repository secret `TAURI_SIGNING_PRIVATE_KEY`; its public half is in `src-tauri/tauri.conf.json`. Keep a backup of the private key. Without it, installed copies can no longer be updated in place.

## Screenshots

The README's screenshots and demo in `docs/media/` were taken from the dev build over the DevTools protocol, at twice the window's size, in both themes (GitHub shows the one matching the reader's), and framed with rounded corners and a shadow. They are lossless WebP; the demo is a GIF. The videos shown are Blender Foundation open movies (CC BY), so they can be shown freely. Retake them when the main screen changes noticeably. When the logo changed (2026-11-28), only the mark in their title bars was redrawn; every other pixel is as taken.
