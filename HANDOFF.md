# Handoff notes

Written at the end of the third build session. This file says where the work stopped and what comes next. Lasting background is elsewhere: architecture in [README.md](README.md), reasons and known issues in [docs/decisions.md](docs/decisions.md), device codec sources in [docs/device-support.md](docs/device-support.md).

When you pick this up: read this file, fold anything lasting into `docs/decisions.md`, then delete it.

## Where things stand

| Phase | Scope | State |
| --- | --- | --- |
| 1 | App shell, managed binaries, link → metadata → download with progress | Done, `v0.1.0` |
| 2 | Device presets and custom mode | Done, `v0.2.0` |
| 3 | Trim with live preview | Done, `v0.3.0` |
| 4 | Download queue, playlists, settings | Done, `v0.4.0` |
| 5 | Update hardening, app self-update, design polish | **In progress** |

`main` is pushed, builds, type-checks and passes 51 Rust tests. The last commit is `feat: keep yt-dlp, FFmpeg and Deno updated in the background`. Do not tag `v0.5.0` until the steps below are done.

## Phase 5

### Done and verified in the running app

- **Daily tool updates.** Checked at launch (the frontend calls `binaries_sync`) and once a day by a loop in `src-tauri/src/main.rs` (`keep_tools_updated`, which wakes hourly and checks when 24 hours have passed).
- **Settings → Updates:** yt-dlp channel (stable or nightly), "install automatically" or "ask first", installed versions, and a **Check now / Update now** button.
- **Toasts** (`src/components/Toaster.tsx`): one line from the release notes after an update, and an offer to update in ask-first mode.
- **No swap while a tool is in use.** Everything that runs a tool holds a shared lock (`BinaryManager::in_use`); the swap needs it exclusively and gives up at once if it can't have it (`ErrorKind::Busy`). Tested: "Update now" during a download left yt-dlp alone and the download kept running; after cancelling, the same button installed the update.
- **Channel switching** in both directions, and the previous version kept in `yt-dlp.previous`.
- **`open_file` / `reveal_file` only accept files downloaded this session.**

### Written but not exercised

- **Update-and-retry on "outdated" errors** (`DownloadManager::run` in `src-tauri/src/downloads/mod.rs`). When a job fails with `ErrorKind::Outdated` (bot check, missing formats, throttling, HTTP 403/429), it updates yt-dlp with `update_now` and runs the job once more. It can only be triggered by a real outdated-engine failure, so it has been reviewed but never run. One way to test it: temporarily make `errors::classify` return `Outdated` for some harmless failure, with an old version written into `manifest.json`.

### Next steps, in order

1. **Ask the owner before the app self-updater.** It needs three things that are theirs to approve:
   - making the GitHub repository public (they chose this approach in session 1, but asked to be asked again before the switch);
   - a signing key pair (`npx tauri signer generate`); the private key must stay out of the repo;
   - storing that private key as a GitHub Actions secret so a release workflow can sign builds.
2. **Add the updater** once approved: `tauri-plugin-updater` and `tauri-plugin-process`, `bundle.createUpdaterArtifacts`, the public key and endpoint `https://github.com/Iosif-Christogeorgos/siphon/releases/latest/download/latest.json` in `tauri.conf.json`, a check at launch with a toast offering to restart, and a GitHub Actions release workflow (`tauri-apps/tauri-action`) triggered by version tags.
3. **Design polish.** I had only located the spots when the session ended; nothing was changed. The list I was about to work through:
   - a sheen on the Download button on hover, and a slight zoom on the thumbnail when hovering the video card;
   - a keyboard hint (`Ctrl+V`) on the empty screen;
   - light-mode screenshots of the trim panel, Downloads panel and Settings to catch contrast problems (only dark mode was checked for these);
   - check the app icon at 32 px, and that "reduce motion" quiets everything (Motion is set to `reducedMotion="user"`; CSS animations use `motion-reduce:`).
4. **Finish:** update the README status table and add an "Updates" section, bump versions to `0.5.0` in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, tag `v0.5.0`, push, and tell the owner how to run and test it.

## Decisions made this session

These are also the candidates to copy into `docs/decisions.md`.

- **Exact cuts still don't use `--force-keyframes-at-cuts`** (decided in session 2, unchanged). That flag makes yt-dlp re-encode video and audio at ffmpeg's defaults on the CPU. Siphon downloads the section losslessly with `--download-sections` and makes the frame-exact cut in its own hardware-accelerated ffmpeg pass. Details are in `docs/decisions.md`.
- **An update that can't swap right away is put off, not queued.** Making it wait would also block new downloads behind it. The daily loop retries within the hour, because a round with a deferred swap doesn't count as "checked".
- **Ask-first mode still installs missing tools.** Only newer versions wait for a go-ahead; without the tools the app can't work at all.
- **Release-note summaries are derived, not copied.** For yt-dlp: the sites listed under "Extractor changes" ("Fixes for youtube, tiktok and 3 more sites."). For Deno: the first bullet. FFmpeg builds have no notes. See `src-tauri/src/binaries/notes.rs`.
- **Only updates get a toast.** First installs are shown by the setup screen.
- **The queue list is ordered as a queue:** running, then waiting in start order, then paused, failed and finished (newest first).
- **The slide-over panels sit inside the error boundary**, so a panel that fails to render shows an error card instead of blanking the window. This came from a real crash (a Zustand selector returning a new array on every call).

## Known issues and findings

- **Intermittent HTTP 403 on trimmed YouTube downloads.** Seen once in session 2: `ffmpeg exited with code 3436169992`, which is ffmpeg's `AVERROR_HTTP_FORBIDDEN`. It succeeded on every retry and from a shell, so it is YouTube refusing a request now and then. It is shown as "The site dropped the connection … Try again" with kind `network`, so the new update-and-retry does **not** cover it. A plain automatic retry for `network` errors would.
- **YouTube draws its own overlay on the paused trim preview** (title, play button, "More videos"). The official embedded player offers no way to hide it.
- **Pausing a trimmed download restarts it on resume.** Section downloads go through ffmpeg inside yt-dlp, which can't continue a partial file.
- **Downloading the same video twice with the same settings overwrites the first file.**
- **The download list is not kept between sessions.**
- **macOS is untested.** Nothing has been built or run there.
- **The shadcn CLI mis-resolves the `@/` alias:** after `npx shadcn add …`, change `import { cn } from "cn"` to `@/lib/utils` and run `npm uninstall cn`.
- **Two browser/API surprises worth remembering:** `scrollIntoView()` now returns a promise, so it must not be the return value of a `useEffect`; and YouTube's embed can stall in "buffering" after a seek, which `YouTubePlayer.tsx` nudges out of.

## How to run and test

```sh
npm install
npm run tauri dev                                  # run the app
npm run typecheck                                  # TypeScript
cargo test --manifest-path src-tauri/Cargo.toml    # 51 Rust tests
npm run tauri build                                # installer in src-tauri/target/release/bundle/nsis
```

If `cargo` isn't found, open a new terminal (Rust was installed in session 1) or add `%USERPROFILE%\.cargo\bin` to `PATH`.

Useful test links:

- `https://www.youtube.com/watch?v=jNQXAC9IVRw` — 19 seconds, quick end-to-end check.
- `https://www.youtube.com/watch?v=aqz-KE-bpKQ` — 10 minutes, 4K60: progress, presets, trimming, pause/resume.
- `https://www.youtube.com/playlist?list=PLa1F2ddGya_86Vmu2clTpAfBvBWnfLc9s` — 20 videos: playlist picker and queue.
- `https://archive.org/details/BigBuckBunny_124` — a non-YouTube site: stream preview, codec-less files.

Where things live on the owner's machine:

- Managed tools: `%LOCALAPPDATA%\app.siphon.desktop\bin` (`manifest.json` holds versions and the last check time).
- Settings: `%APPDATA%\app.siphon.desktop\settings.json`. Testing left it at default values; deleting it is harmless.

To test updates without waiting for a real release: close the app, change `tools.yt_dlp.version` in `manifest.json` to an older value, and start it again. The update installs and the toast appears. In Settings, switching the channel to Nightly and pressing **Check now** does the same from inside the app.

To drive the running app without touching the desktop, start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and use the Chrome DevTools Protocol at `http://127.0.0.1:9222/json` (screenshots, `Runtime.evaluate`, `Input.insertText`, mouse events). `window.__TAURI_INTERNALS__.invoke(…)` calls backend commands directly.
