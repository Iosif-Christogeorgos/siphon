# Decisions and known issues

Why Siphon works the way it does, and what is known not to be right yet. Progress by phase is in the [README](../README.md#status).

## Decisions

### Downloads and quality

- **Copy before convert.** Presets pick a stream the device already plays and re-wrap it. Conversion happens only when the device needs it or the user asks, because converting is slower and never improves quality. The sources for each device's codec support are in [device-support.md](device-support.md).
- **The planner is the single source of truth.** `plan()` in `src-tauri/src/plan/mod.rs` is a pure function from formats, options and trim range to a recipe. The summary shown before downloading and the yt-dlp/ffmpeg commands are both generated from that recipe, so the app cannot describe one thing and do another.
- **Stream ranking** prefers plain files over HLS copies, tracks with known codecs over unknown ones, and audio without levelled ("DRC") volume. These rules came from a real YouTube format list and have regression tests.
- **The share preset defaults to 10 MB** and offers 10, 16, 50, 100 and 500 MB, because the current Discord and WhatsApp limits could not be confirmed (see device-support.md).
- **Format metadata is cached in the backend** by URL (`src-tauri/src/state.rs`), so the frontend never sends formats back.

### Trimming

- **Exact cuts do not use `--force-keyframes-at-cuts`.** Tested: with that flag yt-dlp re-encodes both video and audio with ffmpeg's defaults (the log showed VP9 at CRF 32 and Opus at 96 kbit/s), on the CPU, with no progress. Instead:
  1. yt-dlp downloads only the section with `--download-sections`, stream-copied, plus `--downloader-args ffmpeg_i:-copyts` so the file keeps its original timestamps. It starts at the key frame before the cut.
  2. Siphon's own ffmpeg pass runs with `-copyts`, a `trim=start=…:end=…` video filter on those absolute timestamps and `-output_ts_offset -<start>`. That is one hardware-accelerated video encode, with the audio copied untouched.

  The code is in `src-tauri/src/ytdlp/args.rs` and `src-tauri/src/ffmpeg/mod.rs`. A **fast cut** skips step 2: it is lossless, but starts on a key frame, up to a few seconds early.
- **Each trimmed part is its own download** and its own file, named `Title [id] (1m00s-1m15s).ext`. The backend takes one trim range per job; the UI starts one job per part.
- **An exact cut never changes the resolution a preset picks.** It only turns a copy into an encode: the same codec for H.264 and HEVC sources, otherwise the device's conversion target.
- **Section downloads report no progress** (they go through ffmpeg inside yt-dlp), so progress is read from the growing `.part` file against the expected size.

### Queue and settings

- **Pausing kills the process and keeps the partial files.** yt-dlp has no pause command. Resuming starts the same job again without `--force-overwrites`, so yt-dlp continues from its `.part` file, and a source that was already downloaded goes straight back to converting. Section (trimmed) downloads can't be continued by yt-dlp and start over when resumed.
- **The queue is first in, first out behind one gate.** A job runs when it is first in line and fewer than the configured number are running. The limit is read each time, so changing it in Settings takes effect at once.
- **Settings live in the backend** (`settings.json`), not in browser storage, because the backend is what uses them. The choice of preset and other per-download options stay in the frontend.
- **Custom file names can't contain folders or `..`** and must contain at least one yt-dlp field. The folder is its own setting, and a name without fields would make every download overwrite the last.
- **The download list is not saved between sessions.** A finished file may have been moved or deleted since, and a half-finished one is simply started again.

### Managed binaries

- **FFmpeg uses the `-shared` build** (a 91 MB download rather than 200 MB) and is refreshed at most every 30 days.
- **Each tool lives in its own folder**, so an update is a folder rename and a rollback is the reverse.
- **A tool is never swapped while it is in use.** Everything that runs a tool holds a shared lock (`BinaryManager::in_use`); the swap needs it exclusively and gives up at once if it can't have it.
- **An update that can't swap right away is put off, not queued.** Making it wait would also block new downloads behind it. The daily loop retries within the hour, because a round with a deferred swap doesn't count as "checked".
- **Ask-first mode still installs missing tools.** Only newer versions wait for a go-ahead; without the tools the app can't work at all.
- **Release-note summaries are derived, not copied.** For yt-dlp: the sites listed under "Extractor changes" ("Fixes for youtube, tiktok and 3 more sites."). For Deno: the first bullet. FFmpeg builds have no notes. See `src-tauri/src/binaries/notes.rs`.
- **Only updates get a toast.** First installs are shown by the setup screen.

### Interface

- **The accent colour is aqua/teal.**
- **The download list is ordered as a queue:** running, then waiting in start order, then paused, failed and finished (newest first).
- **The slide-over panels sit inside the error boundary**, so a panel that fails to render shows an error card instead of blanking the window. This came from a real crash: a Zustand selector that returned a new array on every call.
- **`scrollIntoView()` returns a promise**, so it must not be the return value of a `useEffect` (React would call it as a cleanup).
- **`open_file` and `reveal_file` only accept files downloaded this session**, so the webview can't use them to launch arbitrary paths.

### Distribution

- **App self-update uses GitHub Releases from the public repository.**
- **Updates are signed.** The owner approved (2026-11-16) generating a signing key. The private key is at `C:\Users\olimp\.tauri\siphon.key`, outside the repository, and in the GitHub Actions secret `TAURI_SIGNING_PRIVATE_KEY`. It has no password. The public key is in `src-tauri/tauri.conf.json`. If the private key is lost, installed copies can no longer be updated in place, so it needs a backup.
- **Releases are created as drafts.** The owner publishes them on GitHub, and installed copies see nothing until then.
- **The app update is driven from Rust commands** (`src-tauri/src/app_update.rs`), not the updater's JavaScript API, so the webview needs no extra permissions and no extra npm packages.
- **Siphon always asks before updating itself**, whatever the tools' "install automatically" setting says, because it has to restart. It refuses while downloads are running or waiting.

## Testing notes

- **Test links:** `https://www.youtube.com/watch?v=jNQXAC9IVRw` (19 seconds, a quick end-to-end check), `https://www.youtube.com/watch?v=aqz-KE-bpKQ` (10 minutes, 4K60: progress, presets, trimming, pause and resume), `https://www.youtube.com/playlist?list=PLa1F2ddGya_86Vmu2clTpAfBvBWnfLc9s` (20 videos: the playlist picker and the queue), `https://archive.org/details/BigBuckBunny_124` (a site other than YouTube).
- **To test a tool update without waiting for a real release:** close the app, change `tools.yt_dlp.version` in `%LOCALAPPDATA%\app.siphon.desktop\bin\manifest.json` to an older value, and start it again.
- **To drive the running app without touching the desktop:** start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and use the Chrome DevTools Protocol at `http://127.0.0.1:9222/json` (screenshots, `Runtime.evaluate`, `Input.insertText`, mouse events, `Emulation.setEmulatedMedia` for light mode). `window.__TAURI_INTERNALS__.invoke(…)` calls backend commands directly.

## Known issues

- **Intermittent HTTP 403 on section downloads.** A trimmed YouTube download once failed with `ffmpeg exited with code 3436169992`, which is ffmpeg's `AVERROR_HTTP_FORBIDDEN`. The same request succeeded on every retry and from a shell, so it is YouTube refusing a request now and then. It is reported as "The site dropped the connection … Try again"; it is not retried automatically yet. (The update-and-retry for "outdated" errors does not cover it, because it is classed as a network error.)
- **YouTube draws its own overlay on the paused preview.** With the official embedded player, a paused frame is partly covered by the video title, a play button and "More videos". The API offers no way to hide them. Scrubbing still shows the frame underneath.
- **The YouTube embed can stall in "buffering" after a seek.** `YouTubePlayer.tsx` nudges it out with a second seek.
- **Downloading the same video twice with the same settings overwrites the first file.**
- **macOS is untested.** The platform branches exist (VideoToolbox, system FFmpeg, native window buttons), but nothing has been built or run there.
- **The shadcn CLI mis-resolves the `@/` alias.** It writes `import { cn } from "cn"` and installs a package called `cn`. After `npx shadcn add …`, change the import to `@/lib/utils` and run `npm uninstall cn`.
