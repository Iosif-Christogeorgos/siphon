# Decisions and known issues

Why Siphon works the way it does, and what is known not to be right yet. Progress by phase is in the [README](../README.md#status).

## Decisions

### Downloads and quality

- **Copy before convert.** Presets pick a stream the target already plays and re-wrap it. Conversion happens only when the target needs it or the user asks, because converting is slower and never improves quality. What each preset does, and the sources behind it, are in [presets.md](presets.md).
- **Presets are purpose-based** (2026-11-17, replacing ten flat chips): the user picks a group (Watch, Edit, Audio, Max quality, Share), a target in it, and optionally a quality. The groups, targets and tiers are defined once, in `src-tauri/src/plan/registry.rs`; the frontend reads them through `get_presets` and only maps icon names to icons.
- **Decisions come from the video's formats and this machine** (`Capabilities`: working hardware encoders, ffmpeg's `-request_size`, the Windows video extensions installed). The same preset copies on one site and converts on another; tests cover a YouTube-style and an X-style format list.
- **The planner is the single source of truth.** `plan()` in `src-tauri/src/plan/mod.rs` is a pure function from formats, the choice, the trim range and the machine's capabilities to a recipe, including which encoder runs it. The summary shown before downloading and the yt-dlp/ffmpeg commands are both generated from that recipe, so the app cannot describe one thing and do another.
- **Stream ranking** prefers plain files over HLS copies, tracks with known codecs over unknown ones, and audio without levelled ("DRC") volume. These rules came from a real YouTube format list and have regression tests.
- **Upload limits take the cautious side where sources disagree** (see presets.md). Discord Free is 20 MB, confirmed by the owner.
- **Share checks its result.** The finished file is measured against the limit and encoded again at a proportionally lower bitrate if it is over (twice at most, `Recipe::shrunk`); a copied original that came out over is converted with the plan's `fallback`. A video too long to look decent at any size is refused with a suggestion to trim (`ErrorKind::TooLong` if it reaches the download).
- **Edit always re-encodes**, because no metadata says whether web video has a constant frame rate. The intermediate is ProRes 422 rather than DNxHR, because VEGAS's DNxHR import couldn't be confirmed.
- **Android's Best stops at 1080p**; higher is in the quality row. Apple converts to HEVC only with a hardware HEVC encoder, otherwise to H.264 on the GPU, which is far quicker than HEVC on the CPU.
- **The playlist total looks every selected video up**, three at a time (`estimate_download`). The lookups fill the cache the downloads use.
- **Format metadata is cached in the backend** by URL (`src-tauri/src/state.rs`), so the frontend never sends formats back. A download that fails with an "outdated" error looks the video up again on its next try, because the site may have changed what it offers since. The cached copy is replaced, not dropped first, so the open video keeps working if the new lookup fails.

### Download speed

Measured on 2026-11-17 with the 4K test video below, on a connection that downloads a whole YouTube stream at about 52 MiB/s.

- **Trimmed parts are fetched in the request size the site names** (`-request_size` for ffmpeg, from the format's `downloader_options.http_chunk_size`; 10 MB on YouTube). yt-dlp hands a section to ffmpeg, which otherwise asks for the rest of the stream in one request, and YouTube answers that at about twice playback speed: a five-minute 1080p part took 172 s, against 6 s in 10 MB requests, with byte-identical output. Sites that name no size are left alone, because a server that ignores `Range` could misbehave. The option is only passed if the installed ffmpeg knows it (checked once, part of `state::CapabilityCache`): an older ffmpeg stops with "Unrecognized option", which matters on macOS, where ffmpeg comes from Homebrew.
- **Segmented streams download four pieces at once** (`--concurrent-fragments 4`): 23 → 55 MiB/s on a YouTube HLS stream. Eight were no faster, and three downloads at once already means twelve connections to one site.
- **yt-dlp retries five times with a growing pause** (1, 2, 4, 8 and 8 s) instead of its default ten in a row with none, which is over within a second. A short outage is ridden out inside one try; a longer one is left to Siphon's own retries (see Queue and settings). A piece of a segmented stream that can't be fetched fails the try (`--abort-on-unavailable-fragments`) instead of being left out of the file.
- **Not done: reusing the lookup for the download** (`--load-info-json`). It would save about 1.2 s per download, at the risk of stream links that have expired since the lookup.

### Trimming

- **Exact cuts do not use `--force-keyframes-at-cuts`.** Tested: with that flag yt-dlp re-encodes both video and audio with ffmpeg's defaults (the log showed VP9 at CRF 32 and Opus at 96 kbit/s), on the CPU, with no progress. Instead:
  1. yt-dlp downloads only the section with `--download-sections`, stream-copied, plus `--downloader-args ffmpeg_i:-copyts` so the file keeps its original timestamps. It starts at the key frame before the cut.
  2. Siphon's own ffmpeg pass runs with `-copyts`, a `trim=start=…:end=…` video filter on those absolute timestamps and `-output_ts_offset -<start>`. That is one hardware-accelerated video encode, with the audio copied untouched.

  The code is in `src-tauri/src/ytdlp/args.rs` and `src-tauri/src/ffmpeg/mod.rs`. A **fast cut** skips step 2: it is lossless, but starts on a key frame, up to a few seconds early.
- **Section timestamps count from the start of the video** (`-start_at_zero` next to `-copyts`). YouTube's streams start at zero, but Twitch's start about a minute in, so "1:00:00" was timestamp 3659 in the downloaded section. The cut window then held no video frame at all, and an exact cut came out as audio only (found 2026-11-17 on a Twitch VOD).
- **The preview plays HLS streams directly** when the site has no plain file. WebView2 (and Safari) play HLS in a `<video>` element natively, seeking included, and a media element needs no CORS headers (Twitch's CDN sends none, which rules out hls.js). Plain files still come first. Before this, an HLS-only site fell through to the preview copy, which for a seven-hour Twitch VOD meant downloading 2 GB before showing anything.
- **A stream with a bitrate but no size gets one estimated** (bitrate × duration, `estimate_size` in `src-tauri/src/ytdlp/metadata.rs`). yt-dlp usually does this itself, but not for Twitch. Without a size, a trimmed part had nothing to measure its `.part` file against and sat on "Preparing" until it finished.
- **Each trimmed part is its own download** and its own file, named `Title [id] (1m00s-1m15s).ext`. The backend takes one trim range per job; the UI starts one job per part.
- **An exact cut never changes the resolution a preset picks.** It only turns a copy into an encode: the same codec for H.264 and HEVC sources, otherwise the device's conversion target.
- **Section downloads report no progress** (they go through ffmpeg inside yt-dlp), so progress is read from the growing `.part` file against the expected size, every 250 ms. Speed is the file's growth over the last three seconds (ffmpeg writes in bursts, so one step swings wildly), and the time left is the bytes still expected at that speed. Once the file passes the estimated size, the time left is shown as unknown rather than zero.

### Queue and settings

- **Pausing kills the process and keeps the partial files.** yt-dlp has no pause command.
- **A paused or failed download keeps a record of its files** (`leftovers` in `src-tauri/src/downloads/mod.rs`, by download id). The next start under the same id, whether **Resume**, **Try again** or an automatic retry, runs without `--force-overwrites`, so yt-dlp continues from its `.part` file, and a source that was already downloaded goes straight back to converting. A new download runs with `--force-overwrites`, which also makes yt-dlp ignore partial files. The backend decides which applies; the frontend doesn't say. Section (trimmed) downloads are written by ffmpeg, which can't continue a file, so they start over.
- **A download that began by replacing an existing file starts over on every try** (`Leftovers::replaces_existing`). Without `--force-overwrites`, yt-dlp would take the old file for the result and report success.
- **Each stream file says whether it is an intermediate** awaiting a merge, rather than one flag per download. Cleanup deletes intermediates but only the `.part` companions of anything else, so a retry that picks different streams can never delete a finished file. A trimmed part is never an intermediate: it arrives under its final name.
- **The queue is first in, first out behind one gate.** A job runs when it is first in line and fewer than the configured number are running. The limit is read each time, so changing it in Settings takes effect at once.
- **Settings live in the backend** (`settings.json`), not in browser storage, because the backend is what uses them.
- **The last choice and saved presets live in `choices.json`**, also in the backend, separately from `settings.json` so the Settings panel can't overwrite them. Up to 0.6 the choice was in the webview's storage (`siphon.options`); the frontend hands it to `import_legacy_choices` once, and `choices::from_legacy` maps the old preset ids (the only place they still appear). Custom stream picks are not remembered: they belong to one video.
- **A conversion's source waits in a work folder** (`<app cache>/work/<download id>/`) until the finished file is written and checked; the output is written as `Title.converting.ext` and renamed at the end. A failed or paused conversion keeps its source, so **Try again** or **Resume** goes straight back to converting. Work folders are removed on success, on cancel, when the app closes, and at launch if older than a day (newer ones may belong to the other build: dev and installed share the cache folder).
- **Custom file names can't contain folders or `..`** and must contain at least one yt-dlp field. The folder is its own setting, and a name without fields would make every download overwrite the last.
- **Errors say whether they may pass** (`Retry` on `AppError`, which the frontend never sees). A dropped or refused connection, HTTP 403 and 5xx errors get two more tries, after 2 and 10 seconds; HTTP 429 gets one, after 30 (`run` in `src-tauri/src/downloads/mod.rs`). A bot check is not retried, because asking again straight away makes a site more suspicious. An "outdated" error still updates yt-dlp first and tries again with it; if yt-dlp is already current, or the update has to wait for other downloads, it falls back to the pauses above. The wait shows as its own stage ("Trying again in a moment") and ends at once if the download is stopped.
- **The cost of those retries:** being offline takes about 15 s to report instead of 5, and a connection lost in the middle of a download takes about 80 s to give up.
- **The download list is not saved between sessions.** A finished file may have been moved or deleted since, and a half-finished one is simply started again.
- **Closing the app removes partial files** (`DownloadManager::shut_down`). Every download is stopped, which also ends its yt-dlp or ffmpeg process, and what it left is deleted, along with the files of paused and failed downloads. Nothing would ever pick them up again: the list is gone, and a new download starts afresh.

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
- **The theme follows the system unless the user picks one** (Settings → Appearance). Each colour token in `src/index.css` holds both values with `light-dark()`, and `color-scheme` on `<html>` picks one: `light dark` follows the system, and `data-theme="light"` or `"dark"` pins it. One set of tokens instead of two copies, and scrollbars and text fields switch with it. The choice is kept in the webview's storage (`src/stores/theme.ts`), not in `settings.json`, because only the interface uses it, and it is applied before the window is first shown, so there is no flash of the other theme.
- **Settings are grouped lists.** Each setting is one row in a card: its name, with a small control (a switch or stepper) on the same line, and a plain explanation underneath at full width. Wide controls (the folder, the file-name picker) go below the explanation. Squeezing the explanation into a column beside the control made it wrap into four or five ragged lines.
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

- **Presets:** over the DevTools protocol, call `plan_download`, `estimate_download` and `start_download` directly with a `choice`, then check the result with the managed ffprobe (`%LOCALAPPDATA%\app.siphon.desktop\bin\ffmpeg\ffprobe.exe`). To test the migration, delete `%APPDATA%\app.siphon.desktop\choices.json` and put an old state (`{"state":{"preset":"share","sizeLimitMb":50,"mode":"preset","exactCut":true},"version":0}`) in localStorage `siphon.options`. A 4K X video should copy for Apple at Best; none was at hand during the rework, so that case is covered by unit tests only.
- **Test links:** `https://www.youtube.com/watch?v=jNQXAC9IVRw` (19 seconds, a quick end-to-end check), `https://www.youtube.com/watch?v=aqz-KE-bpKQ` (10 minutes, 4K60: progress, presets, trimming, pause and resume), `https://www.youtube.com/playlist?list=PLa1F2ddGya_86Vmu2clTpAfBvBWnfLc9s` (20 videos: the playlist picker and the queue), `https://archive.org/details/BigBuckBunny_124` (a site other than YouTube), `https://www.twitch.tv/videos/2929613567` (a seven-hour Twitch VOD: HLS only, no stated sizes, timestamps that don't start at zero; Twitch deletes old VODs, so replace it when it goes).
- **To test a tool update without waiting for a real release:** close the app, change `tools.yt_dlp.version` in `%LOCALAPPDATA%\app.siphon.desktop\bin\manifest.json` to an older value, and start it again.
- **Retries and lost connections:** serve a file from a small local HTTP server that supports `Range`, can cut the connection after a set number of bytes, and then refuses requests for a set time. yt-dlp's generic extractor downloads `http://127.0.0.1:<port>/clip.mp4` like any other link, and the server's request log shows whether a retry continued (`Range: bytes=N-`) or started over. Point the download folder at a scratch folder first and set it back afterwards.
- **Closing the app:** use the window's Close button (over the DevTools protocol, the button with `aria-label="Close"`). `taskkill /IM siphon.exe` without `/F` did not end the app in testing, so it says nothing about the exit handler.
- **To listen for download events from DevTools:** `const T = window.__TAURI_INTERNALS__; await T.invoke("plugin:event|listen", { event: "download://event", target: { kind: "Any" }, handler: T.transformCallback((e) => console.log(e.payload)) });`
- **To drive the running app without touching the desktop:** start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` and use the Chrome DevTools Protocol at `http://127.0.0.1:9222/json` (screenshots, `Runtime.evaluate`, `Input.insertText`, mouse events, `Emulation.setEmulatedMedia` for light mode). `window.__TAURI_INTERNALS__.invoke(…)` calls backend commands directly.

## Known issues

- **Intermittent HTTP 403 on section downloads.** A trimmed YouTube download once failed with `ffmpeg exited with code 3436169992`, which is ffmpeg's `AVERROR_HTTP_FORBIDDEN`. The same request succeeded on every retry and from a shell, so it is YouTube refusing a request now and then. Since 0.5.2 it is tried twice more (see Queue and settings) before the error is shown.
- **A stalled connection during a trimmed download may hang.** ffmpeg has no read timeout by default and yt-dlp passes none. Adding `-rw_timeout` to the `ffmpeg_i` arguments in `ytdlp/args.rs` would be the fix; untested, so not added. (Siphon's own ffmpeg pass has a marked input-options section in `ffmpeg::convert_args` for the same.)
- **"Update and retry" does little for a queue of downloads.** An update can't be swapped in while other downloads are running (see Managed binaries), so when every video of a playlist fails for the same reason, each falls back to the ordinary retries.
- **HLS trimmed parts print many "keepalive request failed" warnings** from ffmpeg. They are harmless; the download works.
- **YouTube draws its own overlay on the paused preview.** With the official embedded player, a paused frame is partly covered by the video title, a play button and "More videos". The API offers no way to hide them. Scrubbing still shows the frame underneath.
- **The YouTube embed can stall in "buffering" after a seek.** `YouTubePlayer.tsx` nudges it out with a second seek.
- **Downloading the same video twice with the same settings overwrites the first file.** Names are decided in `args::output_template` and, for converted files, `downloads::converted_path`; a unique name would start there.
- **The last-resort preview copy is the whole video.** When neither the embed nor a stream plays, `download_proxy` fetches a 360p copy of everything, which for a long video is gigabytes, with only a spinner meanwhile. It could fetch just a window around the part, or refuse above some length.
- **Dev and installed builds share the app-data folder.** Starting either one clears `%LOCALAPPDATA%\app.siphon.desktop\preview`, and they share `settings.json`. To run the dev build while the installed one is open, give it its own WebView2 profile (`WEBVIEW2_USER_DATA_FOLDER`), or WebView2 fails with `0x8007139F`.
- **macOS has never been run.** The release workflow compiles and bundles it (Apple silicon only, first for v0.5.0), so the code builds, but nobody has started the app there. The platform branches (VideoToolbox, system FFmpeg, native window buttons) are untested, and the bundle is not notarised, so macOS will warn before opening it.
- **The dev server needs the project path spelled as Windows spells it.** Started from `c:\Repos\siphon` (lowercase, as some editors do), Vite bundled React twice and the dev window stayed blank with "Invalid hook call". `vite.config.ts` now sets `root` from `realpathSync.native`, which fixes it. If it ever comes back, delete `node_modules/.vite` and start again from `C:\`.
- **ESLint's TypeScript parser doesn't support TypeScript 7**, so the frontend is linted with oxlint (`npm run lint`, `.oxlintrc.json`): correctness rules, React hooks and unused code. The React Compiler-style rules (`set-state-in-effect`, `refs`) are off; they flag working patterns throughout the app. `npm run knip` finds unused files, exports and dependencies.
- **The shadcn CLI mis-resolves the `@/` alias.** It writes `import { cn } from "cn"` and installs a package called `cn`. After `npx shadcn add …`, change the import to `@/lib/utils` and run `npm uninstall cn`.
