# Siphon

A minimal desktop video downloader for Windows 11 (macOS is supported on a best-effort basis). Siphon is a GUI for [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [FFmpeg](https://ffmpeg.org): paste a link, see what it is, download it.

Siphon is a personal-use tool. It does not bypass DRM; protected videos get a clear message instead of a download.

## Status

Siphon is being built in phases. This is **v0.2.0**.

| Phase | Scope | State |
| --- | --- | --- |
| 1 | App shell, managed binaries, link → metadata → best-quality download with progress | Done |
| 2 | Device presets and manual format selection | Done |
| 3 | Trim with live preview | Planned |
| 4 | Download queue, playlists, settings | Planned |
| 5 | Design polish, background auto-updates, app self-update | Planned |

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

The first launch shows a setup screen while Siphon downloads its engines (about 150 MB). After that, paste a video link, pick where you will watch it, and press **Download**. Files are saved to your Downloads folder.

## Build an installer

```sh
npm run tauri build
```

The installer is written to `src-tauri/target/release/bundle/` (`nsis/` on Windows, `dmg/` on macOS).

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

On launch Siphon asks the GitHub API for the latest release of each tool:

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

### Talking to yt-dlp

All yt-dlp commands are built in Rust and passed as argument arrays, never as shell strings. Links are validated as plain `http(s)` URLs and placed after a `--` separator, so user input can't be read as an option.

Progress comes from `--progress-template` lines that the backend parses and folds into one overall percentage, weighted by stream size. It reaches the UI as `download://event` events.

## Project layout

```
src/                     React + TypeScript frontend
  components/            Shared UI (title bar, error card, shadcn/ui primitives)
  features/              One folder per feature: setup, link, video, options, download, home
  lib/                   IPC wrappers, formatting, motion presets
  stores/                Zustand stores (binaries, downloads, options)
  types.ts               Types mirrored from the Rust backend
src-tauri/               Rust backend (Tauri 2)
  src/binaries/          Download, verify, install and update the managed tools
  src/ytdlp/             Command building, metadata, progress parsing, error classification
  src/plan/              Presets, codec rules and the planner (options → recipe → summary)
  src/ffmpeg/            Hardware encoder detection and conversion arguments
  src/downloads/         Download jobs and progress tracking
  src/commands.rs        The IPC surface
docs/device-support.md   What each preset assumes about its device, with sources
docs/decisions.md        Why things work the way they do, and known issues
app-icon.svg             Icon source; regenerate with `npx tauri icon app-icon.svg`
```

## Stack

Tauri 2, React 19, TypeScript, Vite, Tailwind CSS 4, shadcn/ui, Motion, Zustand, TanStack Query and Lucide icons. There is no telemetry or tracking. The only network requests are the ones needed to fetch a video and to update the managed binaries.
