# Siphon

A minimal desktop video downloader for Windows 11 (macOS is supported on a best-effort basis). Siphon is a GUI for [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [FFmpeg](https://ffmpeg.org): paste a link, see what it is, download it.

Siphon is a personal-use tool. It does not bypass DRM; protected videos get a clear message instead of a download.

## Status

Siphon was built in phases. This is **v0.5.0**, the last planned one.

| Phase | Scope | State |
| --- | --- | --- |
| 1 | App shell, managed binaries, link → metadata → best-quality download with progress | Done |
| 2 | Device presets and manual format selection | Done |
| 3 | Trim with live preview | Done |
| 4 | Download queue, playlists, settings | Done |
| 5 | Background tool updates, app self-update, design polish | Done |

## Requirements

- [Node.js](https://nodejs.org) 20 or newer
- [Rust](https://rustup.rs) (stable)
- **Windows:** Microsoft C++ Build Tools ("Desktop development with C++") and WebView2, which ships with Windows 11
- **macOS:** Xcode Command Line Tools, plus FFmpeg from Homebrew (`brew install ffmpeg`)

You do not need to install yt-dlp, FFmpeg or Deno yourself on Windows. Siphon downloads them on first launch.

## Run it

```sh
npm install
npm run tauri dev
```

The first launch shows a setup screen while Siphon downloads its engines (about 150 MB). After that, paste a video link, pick where you will watch it, and press **Download**. Files are saved to your Downloads folder unless you choose another in Settings.

## Build an installer

```sh
npm run tauri build
```

The installer is written to `src-tauri/target/release/bundle/` (`nsis/` on Windows, `dmg/` on macOS).

The build also signs the installer for the self-update, so it needs the signing key. Without it the build stops at the end with an error about a missing private key. In PowerShell:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.tauri\siphon.key"   # the key file, or its contents
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""                             # the key has no password
npm run tauri build
```

Released versions are not built by hand: see [Releases](#releases).

## Checks

```sh
npm run typecheck                                  # TypeScript
cargo test --manifest-path src-tauri/Cargo.toml    # Rust unit tests
```

## How it works

### Managed binaries

yt-dlp, FFmpeg/ffprobe and Deno (the JavaScript runtime yt-dlp needs for YouTube) are not bundled in the app. They live in the app-data folder so each can update independently:

```
%LOCALAPPDATA%\app.siphon.desktop\bin\
  manifest.json       installed versions
  yt-dlp\             current version
  yt-dlp.previous\    last known-good version, kept for rollback
  ffmpeg\
  deno\
```

At launch, and once a day while it is open, Siphon asks the GitHub API for the latest release of each tool:

| Tool | Source | Checksum file |
| --- | --- | --- |
| yt-dlp | `yt-dlp/yt-dlp` | `SHA2-256SUMS` |
| FFmpeg | `yt-dlp/FFmpeg-Builds` | `checksums.sha256` |
| Deno | `denoland/deno` | `<asset>.sha256sum` |

A new version is downloaded to a staging folder, checked against its published SHA-256, swapped in with a rename, and then started once as a self-test. If the self-test fails, the previous version is put back. FFmpeg's rolling builds are refreshed at most once every 30 days, because they change daily but rarely matter.

On macOS, `yt-dlp/FFmpeg-Builds` publishes no builds, so Siphon uses the FFmpeg it finds on your `PATH` or in Homebrew's folders.

### Presets and the planner

A preset says where the video will be watched (iPhone, Android, Windows 11, Mac, Smart TV), or what it is for (editing, sharing under a size limit, audio only, maximum quality). **Custom** lets you choose the streams, file type and codecs yourself.

The planner in `src-tauri/src/plan` turns that choice into a recipe: which streams to fetch, whether each is copied or converted, and the container. It copies original streams whenever the target can play them, because converting is slower and never improves quality. The summary shown above the Download button is generated from the same recipe that builds the commands, so the app cannot describe one thing and do another.

When a conversion is needed, Siphon runs its own ffmpeg pass with a hardware encoder (NVENC, Quick Sync, AMF or VideoToolbox) if a short test encode shows it works on your machine, and falls back to software otherwise.

What each device can play, and the sources for it, are in [docs/device-support.md](docs/device-support.md).

### Trimming

**Trim** on the video card opens a preview player with a timeline. Drag the two handles, type the times, or use the keyboard, and add more parts if you want several clips. Each part is downloaded as its own file.

| Key | Action |
| --- | --- |
| Space | Play or pause (playback stops at the end of the part) |
| ← / → | Move one second (with Shift: five) |
| , / . | Step one frame back or forward |
| I / O | Set the start or end of the part at the playhead |

The preview comes from YouTube's embedded player for YouTube links, from a low-resolution stream for other sites, and from a small downloaded copy if neither plays.

Only the selected part is downloaded (`--download-sections`). There are two ways to cut it:

- **Exact cut** lands on the precise frames. The video is re-encoded once with a hardware encoder; the audio is left untouched.
- **Fast cut** keeps the original streams and takes no extra time, but starts on a key frame, which can be a few seconds early.

Why Siphon makes exact cuts itself instead of using yt-dlp's `--force-keyframes-at-cuts` is explained in [docs/decisions.md](docs/decisions.md#trimming).

### Queue, playlists and settings

Every download joins a queue. A set number run at once (three by default) and the rest wait their turn, in order. The **Downloads** button in the title bar opens the list, where each download can be paused, resumed, cancelled, retried, opened or shown in its folder.

- **Pause** stops the download but keeps what it has fetched; **Resume** carries on from there.
- **Cancel** stops it and deletes the partial files.
- A download that fails because the connection dropped is tried once more on its own before the error is shown.
- Closing the app pauses whatever is running, so no yt-dlp or ffmpeg process is left behind. The list itself is not kept between sessions.

A playlist link shows its videos with checkboxes. The ones you select are added to the queue with the current preset, and the best format is worked out for each video when its turn comes. A YouTube video link that also names a playlist offers a way to the whole playlist.

**Settings** (the gear in the title bar) holds the download folder, the file-name style, the update choices and the number of simultaneous downloads. They are stored in `settings.json` in the app's config folder (`%APPDATA%app.siphon.desktop` on Windows).

| File-name style | Example |
| --- | --- |
| Title [id] (default) | `Big Buck Bunny [aqz-KE-bpKQ].mp4` |
| Title | `Big Buck Bunny.mp4` |
| Channel - Title | `Blender - Big Buck Bunny.mp4` |
| Date Title | `2014-11-10 Big Buck Bunny.mp4` |
| Custom | Your own yt-dlp template, for example `%(channel)s - %(title)s` |

### Updates

Video sites change often and yt-dlp follows within days, so an old yt-dlp is the usual reason a download stops working. Siphon keeps its tools current on its own.

- **When.** The tools are checked at launch and once a day. **Check now** in Settings → Updates does it on demand.
- **Channel.** yt-dlp comes from its stable releases by default. **Nightly** switches to `yt-dlp/yt-dlp-nightly-builds`, which gets site fixes sooner.
- **Install automatically or ask first.** By default a new version is installed in the background and a toast says what changed. With **Ask first**, the toast offers the update and nothing is replaced until you accept. A tool that is missing is always installed, because Siphon can't work without it.
- **Never mid-download.** A tool is not replaced while a download is using it. The update waits and is tried again within the hour.
- **Update and retry.** If a download fails in a way that points to an outdated yt-dlp (a bot check, an unreadable page, HTTP 403 or 429), Siphon looks for a newer yt-dlp and, if there is one, installs it and tries the download once more before reporting the error.

Siphon also updates itself. At launch it looks for a newer published release on GitHub. If there is one, a toast and Settings → Updates offer **Restart to update**. Siphon always asks first, because it has to restart, and it refuses while downloads are running or waiting. The update is only installed if its signature matches the public key built into the app.

### Releases

Pushing a version tag (`v0.5.0`) runs `.github/workflows/release.yml`. It builds and signs the app, and attaches the installer, its signature and `latest.json` (the file installed copies read to find updates) to a **draft** GitHub release. Nothing reaches installed copies until the draft is published on GitHub.

To make a release: set the same version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, then tag and push.

The signing key is in the repository secret `TAURI_SIGNING_PRIVATE_KEY`; its public half is in `src-tauri/tauri.conf.json`. Keep a backup of the private key. Without it, installed copies can no longer be updated in place.

### Talking to yt-dlp

All yt-dlp commands are built in Rust and passed as argument arrays, never as shell strings. Links are validated as plain `http(s)` URLs and placed after a `--` separator, so user input can't be read as an option.

Progress comes from `--progress-template` lines that the backend parses and folds into one overall percentage, weighted by stream size. It reaches the UI as `download://event` events.

## Project layout

```
src/                     React + TypeScript frontend
  components/            Shared UI (title bar, error card, shadcn/ui primitives)
  features/              One folder per feature: setup, link, video, playlist, options, trim,
                         download, queue, settings, home
  lib/                   IPC wrappers, formatting, motion presets
  stores/                Zustand stores (binaries, downloads, options, trim, toasts, appUpdate, ui)
  types.ts               Types mirrored from the Rust backend
src-tauri/               Rust backend (Tauri 2)
  src/binaries/          Download, verify, install and update the managed tools
  src/ytdlp/             Command building, metadata, progress parsing, error classification
  src/plan/              Presets, codec rules and the planner (options → recipe → summary)
  src/ffmpeg/            Hardware encoder detection and conversion arguments
  src/downloads/         The download queue, jobs (pause, resume, cancel) and progress tracking
  src/settings.rs        User preferences and where they are stored
  src/app_update.rs      Checking for and installing a newer Siphon
  src/preview.rs         Where the trim preview plays from (embed, stream or cached copy)
  src/commands.rs        The IPC surface
.github/workflows/release.yml   Builds and signs a draft release when a version tag is pushed
docs/device-support.md   What each preset assumes about its device, with sources
docs/decisions.md        Why things work the way they do, and known issues
app-icon.svg             Icon source; regenerate with `npx tauri icon app-icon.svg`
```

## Stack

Tauri 2, React 19, TypeScript, Vite, Tailwind CSS 4, shadcn/ui, Motion, Zustand, TanStack Query and Lucide icons. There is no telemetry or tracking. The only network requests are the ones needed to fetch a video and to update Siphon and its managed binaries.
