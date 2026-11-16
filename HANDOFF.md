# Handoff notes

Written at the end of the fourth build session. This file says where the work stopped and what comes next. Lasting background is elsewhere: architecture in [README.md](README.md), reasons and known issues in [docs/decisions.md](docs/decisions.md), device codec sources in [docs/device-support.md](docs/device-support.md), working rules in [CLAUDE.md](CLAUDE.md).

When you pick this up: read this file, fold anything lasting into `docs/decisions.md`, then delete it.

## Where things stand

| Phase | Scope | State |
| --- | --- | --- |
| 1 | App shell, managed binaries, link → metadata → download with progress | Done, `v0.1.0` |
| 2 | Device presets and custom mode | Done, `v0.2.0` |
| 3 | Trim with live preview | Done, `v0.3.0` |
| 4 | Download queue, playlists, settings | Done, `v0.4.0` |
| 5 | Update hardening, app self-update, design polish | **In progress, close to done** |

`main` is pushed, builds, type-checks and passes 51 Rust tests. The last code commit is `feat: update Siphon itself from signed GitHub releases`. Versions in the source still say `0.4.0`. Do not tag `v0.5.0` until the steps below are done.

## Phase 5

### Done and verified in the running app

- **Background tool updates** (yt-dlp, FFmpeg, Deno): daily check, stable/nightly channel, ask-first mode, release-note toasts, no swap while a download uses a tool. Verified last session.
- **Update-and-retry on "outdated" errors.** Exercised this session with a temporary hook (a 404 classed as outdated, since reverted): the job reported the stage `updating`, yt-dlp went from the old version to the current one, the toast appeared, and a second attempt ran.
- **App self-update, as far as it can go without a release:**
  - `src-tauri/src/app_update.rs` has two commands, `app_update_check` and `app_update_install`. With no published release, the check returns "no update" without an error (tested in dev).
  - Settings → Updates shows a "Siphon 0.4.0" row; a newer version adds a **Restart to update** button there and in a toast.
  - Installing is refused while downloads are running or waiting.
- **Polish, in dark mode:** a `Ctrl V` hint under the paste field on the empty screen (seen in a screenshot).

### Written but not checked

- **The release workflow** (`.github/workflows/release.yml`) has never run. A pushed `v*` tag should build and sign Windows and macOS bundles and attach them, with `latest.json`, to a **draft** release. The macOS job is there mainly as a first compile test on macOS; it may fail.
- **A real app update** (an older installed Siphon finding a published release, installing it and restarting).
- **A local release build.** `tauri.conf.json` now sets `bundle.createUpdaterArtifacts`, so `npm run tauri build` needs the signing key. Expected to work with `TAURI_SIGNING_PRIVATE_KEY` set to the path `C:\Users\olimp\.tauri\siphon.key` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` set to an empty string. Not tried.
- **Polish that was applied but not looked at:** the light sweep across the Download button on hover, the thumbnail zoom when hovering the video card, and the global "reduce motion" rule in `src/index.css`.
- **Light mode** for the trimmer, the Downloads panel and Settings. Screenshots were taken but not reviewed before the session ended; take them again.

### Next steps, in order

1. Run the app (`npm run tauri dev`) and look at the unchecked polish above, in dark and light mode. Fix any contrast or layout problems. Also check the app icon at 32 px.
2. Try a local release build with the signing key (see above). If it needs something else, document it in the README.
3. Update the README: mark phase 5 done in the status table, add an "Updates" section (tool updates, channels, ask-first, update-and-retry, app self-update, how releases are made and published), and note the signing key requirement under "Build an installer". Add `src/app_update.rs` and `.github/workflows/release.yml` to the project layout.
4. Bump the version to `0.5.0` in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` (then `npm install --package-lock-only` and a `cargo check` to refresh the lock files). Commit, tag `v0.5.0`, push with `--follow-tags`.
5. Watch the workflow (`gh run watch`). Fix what fails. If the macOS job can't be made to pass quickly, remove it from the matrix and say so in `docs/decisions.md`.
6. Check the draft release has the installer, its `.sig` and `latest.json`. **Do not publish it**: the owner chose to publish drafts themselves. Tell them it is waiting, and that installed copies only see an update once a release is published.
7. Tell the owner how to run and test phase 5, and that the self-update can only be proven end to end with a second release (for example `v0.5.1`) after `v0.5.0` is installed.

## Decisions made this session

Candidates to copy into `docs/decisions.md`:

- **The owner approved** (2026-11-16) generating a signing key and storing the private key as the GitHub Actions secret `TAURI_SIGNING_PRIVATE_KEY`. They made the repository public themselves.
- **Releases are created as drafts.** The owner chose "draft first": nothing reaches installed copies until they press Publish on GitHub.
- **The signing key has no password.** The private key is at `C:\Users\olimp\.tauri\siphon.key` (outside the repo) and in the GitHub secret. The public key is in `src-tauri/tauri.conf.json`. Losing the private key means installed copies can no longer be updated in place; back it up.
- **The app update is driven from Rust commands**, not the updater's JavaScript API, so the webview needs no extra permissions and no extra npm packages.
- **Siphon always asks before updating itself**, whatever the tools' "install automatically" setting says, because it has to restart.
- **Exact cuts still don't use `--force-keyframes-at-cuts`** (decided in session 2, unchanged; see `docs/decisions.md`).

## Known issues and findings

All earlier ones are in `docs/decisions.md`. Still open and worth knowing:

- **Intermittent HTTP 403 on trimmed YouTube downloads** is classed as a network error, so update-and-retry does not cover it. A plain automatic retry for network errors would.
- **macOS is untested.** The release workflow's macOS job will be the first time the code is compiled there.
- **The shadcn CLI mis-resolves the `@/` alias:** after `npx shadcn add …`, change `import { cn } from "cn"` to `@/lib/utils` and run `npm uninstall cn`.

## How to run and test

```sh
npm install
npm run tauri dev                                  # run the app
npm run typecheck                                  # TypeScript
cargo test --manifest-path src-tauri/Cargo.toml    # 51 Rust tests
```

If `cargo` isn't found, open a new terminal or add `%USERPROFILE%\.cargo\bin` to `PATH`.

Useful test links:

- `https://www.youtube.com/watch?v=jNQXAC9IVRw` — 19 seconds, quick end-to-end check.
- `https://www.youtube.com/watch?v=aqz-KE-bpKQ` — 10 minutes, 4K60: progress, presets, trimming, pause/resume.
- `https://www.youtube.com/playlist?list=PLa1F2ddGya_86Vmu2clTpAfBvBWnfLc9s` — 20 videos: playlist picker and queue.
- `https://archive.org/details/BigBuckBunny_124` — a non-YouTube site.

Where things live on the owner's machine:

- Managed tools: `%LOCALAPPDATA%\app.siphon.desktop\bin` (`manifest.json` holds versions and the last check time).
- Settings: `%APPDATA%\app.siphon.desktop\settings.json`. It was deleted at the end of testing, so the app starts with defaults.

To test tool updates without waiting for a real release: close the app, change `tools.yt_dlp.version` in `manifest.json` to an older value, and start it again.

To drive the running app without touching the desktop, start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and use the Chrome DevTools Protocol at `http://127.0.0.1:9222/json` (screenshots, `Runtime.evaluate`, `Input.insertText`, mouse events, `Emulation.setEmulatedMedia` for light mode). `window.__TAURI_INTERNALS__.invoke(…)` calls backend commands directly.
