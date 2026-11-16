# Handoff notes

Written 2026-11-16 at the end of the first build session. This file is for whoever continues the work with no memory of that session. The original brief is summarised under [The brief](#the-brief); the README covers architecture.

## Where things stand

| Phase | Scope | State |
| --- | --- | --- |
| 1 | App shell, managed binaries, link → metadata → download with progress | Done, tagged `v0.1.0` |
| 2 | Device presets and custom (manual) mode | Done, tagged `v0.2.0` |
| 3 | Trim with live preview | **In progress** (backend done, UI unverified) |
| 4 | Download queue, playlists, settings | Not started |
| 5 | Design polish, background auto-updates, app self-update | Not started |

`main` builds, type-checks and passes all 41 Rust tests at the last commit. Do not tag `v0.3.0` until the phase 3 checklist below is done.

## Phase 3: exactly where it stopped

### Done and verified

- **Trim backend** (commit `feat: download only a selected part of a video`). A `trim: { start, end, exact }` rides along with any preset or custom choice, in both `plan_download` and `start_download`.
- Verified with real downloads and ffprobe: an exact cut of 15 s at 60 fps produced exactly 900 frames; a 4K exact cut, a single-combined-file source, a fast cut and an audio-only cut all worked.
- **Preview backend** (`src-tauri/src/preview.rs`, command `preview_source(url, level)`). It compiles; only level 0 (YouTube) has been exercised.

### Written but not verified (commit `wip: trim panel …`)

The trimmer UI in `src/features/trim/` and its wiring in `src/features/video/VideoPane.tsx`. What was checked: the panel opens from the **Trim** button on the video card, the YouTube preview loads, and Play advances the time readout. Nothing else was tested.

### Next steps, in order

1. Run the app, open the trimmer on a YouTube video, and test by hand:
   - dragging both handles and the playhead (the frame under a handle should show while dragging);
   - typing times into the fields, including ↑/↓ nudging;
   - shortcuts: Space, ←/→ (Shift for 5 s), `,` and `.` (one frame), `I` and `O`;
   - **Add another part**, removing a part, and the **Exact cut / Fast cut** toggle;
   - pressing **Download clip** and **Download N clips**, and that the plan summary and size estimate follow the active part.
2. Test the preview on a non-YouTube site (for example Vimeo), which uses level 1 (direct stream) and, if that fails, level 2 (a ~360p proxy downloaded to the cache folder and served through the asset protocol). Neither level has been run.
3. Check the YouTube embed in a **release** build (`npm run tauri build`, then run the exe). It worked in dev, where the page origin is `http://localhost:1420`. In release the origin is `http://tauri.localhost`, and YouTube embeds are known to fail with "error 153" in some webviews. If it fails, the code already falls through to levels 1 and 2.
4. Known rough edge to look at: when the YouTube preview is paused, YouTube draws its own overlay (title, "More videos") over the frame. It is cosmetic, but it gets in the way of judging a frame. Options: accept it, or prefer the stream/proxy source for scrubbing.
5. Update the README (status table, a "Trimming" section), bump versions to `0.3.0` in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, tag `v0.3.0`, push.
6. Then phase 4 and phase 5 as described in the brief.

## Decisions made, and why

- **Exact cuts do not use `--force-keyframes-at-cuts`**, although the brief asked for it. Tested: with that flag yt-dlp re-encodes *both* video and audio with ffmpeg's defaults (the log showed VP9 at CRF 32 and Opus at 96 kbit/s), on the CPU, and reports no progress. Instead:
  1. yt-dlp downloads only the section with `--download-sections`, stream-copied, plus `--downloader-args ffmpeg_i:-copyts` so the file keeps its original timestamps (it starts at the key frame before the cut).
  2. Siphon's own ffmpeg pass runs with `-copyts`, a `trim=start=…:end=…` video filter on those absolute timestamps, `-output_ts_offset -<start>`, one hardware-accelerated video encode, and audio copied untouched.
  The code is in `src-tauri/src/ytdlp/args.rs` and `src-tauri/src/ffmpeg/mod.rs`. A **fast cut** skips step 2: lossless, but it starts on a key frame, up to a few seconds early.
- **Each trimmed part is its own download job** and its own file, named `Title [id] (1m00s-1m15s).ext`. The backend takes one `trim` per job; the UI starts one job per part.
- **An exact cut never changes the resolution a preset picks.** It only changes copy into encode: same codec for H.264/HEVC sources, otherwise the device's conversion target.
- **Section downloads report no progress** (they go through ffmpeg inside yt-dlp), so progress is read from the growing `.part` file against the expected size.
- **The planner is the single source of truth.** `plan()` in `src-tauri/src/plan/mod.rs` is a pure function from formats + options + trim to a recipe. The summary text and the commands are both generated from that recipe.
- **Copy before convert.** Presets pick a natively playable stream and remux; conversion happens only when the device needs it or the user asks. Sources for each device's codec support are in `docs/device-support.md`.
- **Stream ranking** prefers plain files over HLS copies, described tracks over codec-less ones, and non-"DRC" audio. These came from a real YouTube format list and have regression tests.
- **The share preset defaults to 10 MB** and offers 10/16/50/100/500 MB, because Discord's and WhatsApp's current limits could not be confirmed (sources conflicted; details in `docs/device-support.md`).
- **FFmpeg uses the `-shared` build** (91 MB download rather than 200 MB) and is refreshed at most every 30 days. Each tool lives in its own folder so an update is a folder rename.
- **Format metadata is cached in the backend** by URL (`state.rs`), so the frontend never sends formats back.
- **Accent colour is aqua/teal** (the owner's choice).
- **App self-update will use GitHub Releases from a public repo** (the owner's choice). The repo is still private. Changing its visibility needs the owner's explicit go-ahead when phase 5 is reached.

## Known issues and findings

- **Intermittent HTTP 403 on section downloads.** One trimmed YouTube download failed with `ffmpeg exited with code 3436169992`. That number is ffmpeg's `AVERROR_HTTP_FORBIDDEN` (the bytes spell `F8 '4' '0' '3'`). The same request succeeded on every retry and from a shell, so it is YouTube refusing a request now and then, not the app's process setup. It is now reported as "The site dropped the connection … Try again". Not done: retrying automatically. Phase 5's "retry once" logic is the natural place.
- **Binary updates can race a download.** The update check at launch may swap yt-dlp's folder while a download is starting. Phase 5 should defer updates while jobs run.
- **The same video downloaded twice with the same output name overwrites** (`--force-overwrites`). Phase 4's filename templates should replace this.
- **`open_file`/`reveal_file` take any existing path** from the webview. Acceptable for a personal tool, but worth restricting to known download paths in phase 5.
- **macOS is untested.** The code has the platform branches (VideoToolbox, system ffmpeg, native traffic lights), but nothing was built or run there.
- **The shadcn CLI mis-resolves the `@/` alias** (it writes `import { cn } from "cn"` and installs a package called `cn`). After `npx shadcn add …`, fix the import to `@/lib/utils` and run `npm uninstall cn`.

## How to run and test

Requirements are in the README. On the owner's machine everything is installed; open a new terminal so `cargo` is on PATH.

```sh
npm install
npm run tauri dev                                  # run the app
npm run typecheck                                  # TypeScript
cargo test --manifest-path src-tauri/Cargo.toml    # 41 Rust tests
npm run tauri build                                # installer in src-tauri/target/release/bundle/nsis
```

Manual test links: `https://www.youtube.com/watch?v=jNQXAC9IVRw` (19 s, quick) and `https://www.youtube.com/watch?v=aqz-KE-bpKQ` (10 min, 4K60, good for progress, presets and trimming). The managed binaries are in `%LOCALAPPDATA%\app.siphon.desktop\bin`; delete that folder to see the first-run setup screen.

To drive the running app without touching the desktop, start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and use the Chrome DevTools Protocol at `http://127.0.0.1:9222/json` (screenshots, `Runtime.evaluate`, `Input.insertText`). Calling `window.__TAURI_INTERNALS__.invoke("start_download", …)` from there runs a job end to end.

## The brief

Siphon is a personal-use desktop video downloader for Windows 11 (macOS a bonus) wrapping yt-dlp and ffmpeg. Looks and feel come first: minimal, premium, one accent colour, system light/dark, smooth animations that respect "reduce motion". It must never try to bypass DRM.

Still to build from the brief:

- **Phase 4:** a download queue (several at once, pause/cancel/retry, open file/folder), a remembered output folder, filename templates, playlist detection with a picker for which videos to download (the backend already returns playlists; the UI shows a placeholder), and a settings screen.
- **Phase 5:** check for binary updates on launch and every 24 h; stable/nightly channel and auto/ask-first settings; a toast with a one-line summary from the release notes; on a known "outdated" error (bot check, format unavailable, throttling) update yt-dlp and retry once; the Tauri updater plugin for the app itself; animation and design polish; a final pass on the app icon.

Working rules from the brief: Conventional Commits, one small commit per finished step that leaves the app working, push after each phase, tag each phase (`v0.3.0`, …), never commit secrets or downloaded media, no telemetry, no unused code or dependencies, and tell the owner how to run and test after each phase.
