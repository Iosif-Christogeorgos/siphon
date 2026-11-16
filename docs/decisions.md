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

### Product

- **The accent colour is aqua/teal.**
- **App self-update will use GitHub Releases from a public repository.** The repository is private until its owner says otherwise.

## Known issues

- **Intermittent HTTP 403 on section downloads.** A trimmed YouTube download once failed with `ffmpeg exited with code 3436169992`, which is ffmpeg's `AVERROR_HTTP_FORBIDDEN`. The same request succeeded on every retry and from a shell, so it is YouTube refusing a request now and then. It is reported as "The site dropped the connection … Try again"; it is not retried automatically yet.
- **YouTube draws its own overlay on the paused preview.** With the official embedded player, a paused frame is partly covered by the video title, a play button and "More videos". The API offers no way to hide them. Scrubbing still shows the frame underneath.
- **Binary updates can race a download.** The update check at launch may swap yt-dlp's folder while a download is starting.
- **Downloading the same video twice with the same settings overwrites the first file.**
- **`open_file` and `reveal_file` accept any existing path** from the webview. Acceptable for a personal tool, but worth restricting to known download paths.
- **macOS is untested.** The platform branches exist (VideoToolbox, system FFmpeg, native window buttons), but nothing has been built or run there.
- **The shadcn CLI mis-resolves the `@/` alias.** It writes `import { cn } from "cn"` and installs a package called `cn`. After `npx shadcn add …`, change the import to `@/lib/utils` and run `npm uninstall cn`.
