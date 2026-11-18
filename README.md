# Siphon

A minimal desktop video downloader for Windows 11 (Windows only). Siphon is a GUI for [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [FFmpeg](https://ffmpeg.org): paste a link, see what it is, download it.

Siphon is a personal-use tool. It does not bypass DRM; protected videos get a clear message instead of a download. Videos that need an account can be downloaded by signing in with your own, which unlocks only what that account can watch.

## Status

Siphon was built in phases, and all five are done. This is **v0.11.0**.

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
- Microsoft C++ Build Tools ("Desktop development with C++") and WebView2, which ships with Windows 11

Siphon is Windows only. You do not need to install yt-dlp, FFmpeg or Deno yourself: Siphon downloads them on first launch.

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

The installer is written to `src-tauri/target/release/bundle/nsis/`.

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
npm run lint                                       # oxlint: correctness, React hooks, unused code
npm run knip                                       # unused files, exports and dependencies
cargo test --manifest-path src-tauri/Cargo.toml    # Rust unit tests
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

GitHub runs all of them (`.github/workflows/ci.yml`) on every push to `main` and every pull request, and the release workflow runs them first, so a release is never built from a commit that fails them.

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

### Presets and the planner

Presets start from where the video is going. Pick a **group**, then a **target** in it, and optionally a **quality**:

| Group | Targets | What you get |
| --- | --- | --- |
| Max quality | | The best streams exactly as the site has them; may not play everywhere |
| Watch | Windows · iPhone / iPad / Mac · Android · TV | The best version that plays natively on the device |
| Share | Discord · WhatsApp · TikTok / Reels / Shorts | The best quality that fits the upload limit of your tier |
| Audio | Music · Best audio (original) | MP3 or M4A at a chosen bitrate, or the original track untouched |
| Edit | Premiere Pro · DaVinci Resolve · CapCut · Vegas | Constant frame rate H.264 with 48 kHz sound; or ProRes with **Smoothest editing (large files)** |

The **Quality** choice offers **Best** and the resolutions the video actually has. It shows only the current pick until you point at it; it then unfolds across the row, and each resolution shows its estimated size as you point at it (a range when converting, since the size then depends on the picture); a lower one downloads the matching stream, and nothing is upscaled. Music offers bitrates instead; Max quality and Share have no quality row. **Customize** (under the summary) opens the old manual mode, where you choose the streams, file type and codecs yourself, and can save them as a named preset that appears under **My presets**. Siphon remembers the last group, target, quality and Share tier.

The presets are defined once, in `src-tauri/src/plan/registry.rs`, and the interface reads them from the backend. The planner in `src-tauri/src/plan` turns a choice into a recipe from the formats the video really offers: the same Apple preset copies X's 4K H.264 file but converts YouTube's 4K VP9 to HEVC. It copies original streams whenever the target can play them, because converting is slower and never improves quality. The summary above the Download button is generated from the same recipe that builds the commands, so the app cannot describe one thing and do another. It marks each plan **No quality loss**, **Converts** (with a rough time), **Size-optimized** or **May not play everywhere**.

When a conversion is needed, Siphon runs its own ffmpeg pass with a hardware encoder (NVENC, Quick Sync or AMF) if a short test encode shows it works on your machine, and falls back to software otherwise. The downloaded source waits in a work folder until the finished file is written and checked, so a failed conversion can be tried again without downloading again.

**Share** works out the bitrate from the limit and the length (the trimmed length, if you trimmed), picks the size and frame rate that look best for it (a clean 720p30 rather than a blocky 1080p60), and checks the finished file, encoding again a little smaller if it came out over. An original that already fits is copied as it is. A video too long to look decent under the limit isn't made at all: Siphon suggests trimming it instead.

What each target plays or needs, the upload limits, and the sources for them are in [docs/presets.md](docs/presets.md).

### Trimming

**Trim** on the video card opens a preview player with a timeline. Drag the two handles, type the times, or use the keyboard, and add more parts if you want several clips. Each part is downloaded as its own file.

| Key | Action |
| --- | --- |
| Space | Play or pause (playback stops at the end of the part) |
| ← / → | Move one second (with Shift: five) |
| , / . | Step one frame back or forward |
| I / O | Set the start or end of the part at the playhead |

The preview comes from YouTube's embedded player for YouTube links, from a low-resolution stream for other sites (a plain file if the site has one, otherwise HLS, which is all Twitch offers), and from a small low-resolution copy if neither plays.

Only the selected part is downloaded (`--download-sections`), and every clip starts and ends on the exact frames chosen. There is no choice to make:

- **When the stream is kept as it is** (H.264 or HEVC), Siphon makes a **smart cut**: it re-encodes only the frames from the start of the clip to the next key frame and from the last key frame to the end, about two seconds each, and copies everything in between untouched. The re-encoded frames match the original's settings and are visually lossless. An hour of 1080p60 from Twitch is cut in about 20 seconds.
- **When the video is converted anyway** (Edit, Share at a lower size, VP9 or AV1 kept as they are, or a clip for iPhone / iPad / Mac), the cut is made during that conversion.

Every smart cut is checked before it is kept: every frame at its time, the sound in sync from start to end, and the joins decoding exactly like the original. If anything is off, the whole clip is re-encoded instead, so a clip never comes out broken. `siphon.log` (in `%LOCALAPPDATA%\app.siphon.desktop\logs`) records which way each cut was made. How it works, and what was measured, is in [docs/decisions.md](docs/decisions.md#trimming).

### Queue, playlists and settings

Every download joins a queue. A set number run at once (three by default) and the rest wait their turn, in order. The **Downloads** button in the title bar opens the list, where each download can be paused, resumed, cancelled, retried, opened or shown in its folder.

- **Pause** stops the download but keeps what it has fetched; **Resume** carries on from there.
- **Cancel** stops it and deletes the partial files.
- A download that fails because the connection dropped or the site refused it is tried again on its own: twice, after 2 and 10 seconds. A site that is limiting requests (HTTP 429) gets one more try, after 30 seconds. The download shows "Trying again in a moment" while it waits.
- Every new try, automatic or with **Try again**, carries on from what the last one fetched instead of starting over.
- A trimmed part whose connection stops sending gives up after 20 seconds and is tried again, instead of waiting forever.
- Downloading something again never replaces or touches a file already in the folder: the new one is named `Title [id] (2).mp4`, then `(3)`, and so on. While a download runs, its files carry a `.siphon-…` marker in their name, which comes off when it is finished.
- Opening a video you have downloaded before says so under its card ("You already have this · today · 860 MB", or "a clip of this"), with **Open** and **Show in folder**, as long as the file is still where Siphon put it. A video that is downloading or waiting from elsewhere (a playlist, say) shows "Already in your downloads".
- When downloads finish or fail while Siphon isn't the active window, Windows shows a notification. Downloads that end within two seconds of each other share one ("3 downloads finished").
- Closing the app pauses every download, so no yt-dlp or ffmpeg process is left behind; while any are running or waiting, Siphon asks first. The list is kept between sessions: paused downloads come back paused and carry on from where they stopped (**Resume all** resumes them together), and finished ones can still be opened.

**Speed limit.** Settings → Downloads → **Speed limit** caps the total download speed of all downloads together: Unlimited (the default), 20, 30, 40 or 50 MB/s, or a custom value. Running downloads share it evenly, a download that needs less leaves the rest to the others, and a change applies to running downloads within a second. The limit is kept in `settings.json`, so it stays on until it is changed. While it is on, the title bar says so ("Limited to 20 MB/s"; click it to change it), and while anything downloads it shows the total speed. MB here is 1,048,576 bytes, the unit download speeds are shown in. How it works is under [Talking to yt-dlp](#talking-to-yt-dlp).

A playlist link shows its videos with checkboxes. The ones you select are added to the queue with the current preset, and the best format is worked out for each video when its turn comes. The estimated total size comes from looking up the first 15 selected videos and scaling them by length to the rest. A YouTube video link that also names a playlist offers a way to the whole playlist.

Videos you have already downloaded (and whose file is still there) are marked **Downloaded** and start unselected, and videos already in the queue are marked **In queue** and can't be added twice, so pasting a playlist again fetches only what's new. Below the list, **Save in a folder named …** puts the videos in a folder named after the playlist, and **Number the files in playlist order** names them `01 - Title.mp4`, `02 - …` (three digits for a playlist of a hundred or more); both choices are remembered. Shift-click selects or unselects everything between two videos, and a list of ten or more has **Find by title**.

A channel link (`youtube.com/@name`) lists the channel's videos grouped as Videos, Live and Shorts, with Videos selected. A channel's Playlists tab lists its playlists; clicking one opens it. Streams that are live or haven't started yet are left out until they can be downloaded.

**Settings** (the gear in the title bar) holds the download folder, the file-name style, the number of simultaneous downloads, the speed limit, the theme and the update choices. They are stored in `settings.json` in the app's config folder (`%APPDATA%\app.siphon.desktop` on Windows), except the theme, which only the interface uses and which is kept with it. The last preset choice and your saved presets are in `choices.json` next to it.

The theme follows Windows' light or dark mode by default (**System**), and changes when Windows does. **Light** or **Dark** keeps Siphon in that theme whatever Windows uses.

| File-name style | Example |
| --- | --- |
| Title [id] (default) | `Big Buck Bunny [aqz-KE-bpKQ].mp4` |
| Title | `Big Buck Bunny.mp4` |
| Channel - Title | `Blender - Big Buck Bunny.mp4` |
| Date Title | `2014-11-10 Big Buck Bunny.mp4` |
| Custom | Your own yt-dlp template, for example `%(channel)s - %(title)s` |

### Signing in

When a video needs an account (age-restricted, members-only, a post only signed-in viewers see), the error says which site ("**youtube.com** needs you to sign in") and offers **Sign in**, with the way to sign in beside it:

- **Sign-in window**: the site's own sign-in page in a Siphon window, with its own browser profile. Sign in as usual and press **Done**; on YouTube, Instagram, X and a few others it finishes by itself.
- **A browser on this computer**: takes the sign-in over from it. Firefox works; Chrome, Edge and Brave on Windows encrypt their cookies so only they can read them, and Siphon says so.
- **cookies.txt**: a cookies file exported from your browser with an extension.

Siphon checks the sign-in on the video before saying it worked, then looks the video up again and retries that site's failed downloads. Each site's cookies are kept apart, encrypted for your Windows account, and given to that site's downloads only, in a temporary file deleted when the download ends. Private, paid and password-protected videos don't offer sign-in, because an account doesn't help with those. A sign-in that has run out says so. **Settings → Signed-in sites** lists them, with **Sign out**. Details, including why Google sometimes refuses embedded sign-in windows, are in [docs/sign-in.md](docs/sign-in.md).

### Updates

Video sites change often and yt-dlp follows within days, so an old yt-dlp is the usual reason a download stops working. Siphon keeps its tools current on its own.

- **When.** The tools are checked at launch and once a day. **Check now** in Settings → Updates does it on demand.
- **Channel.** yt-dlp comes from its stable releases by default. **Nightly** switches to `yt-dlp/yt-dlp-nightly-builds`, which gets site fixes sooner.
- **Install automatically or ask first.** By default a new version is installed in the background and a toast says what changed. With **Ask first**, the toast offers the update and nothing is replaced until you accept. A tool that is missing is always installed, because Siphon can't work without it.
- **Never mid-download.** A tool is not replaced while a download is using it. The update waits and is tried again within the hour.
- **Update and retry.** If a download fails in a way that points to an outdated yt-dlp (a bot check, an unreadable page, HTTP 403 or 429), Siphon looks for a newer yt-dlp and, if there is one, installs it and tries the download once more before reporting the error. If there is none, a refused or limited request is still tried again after a pause (see [Queue](#queue-playlists-and-settings)), and the video is looked up afresh in case the site changed what it offers.

Siphon also updates itself. At launch it looks for a newer published release on GitHub. If there is one, a toast and Settings → Updates offer **Restart to update**. Siphon always asks first, because it has to restart, and it refuses while downloads are running or waiting. The update is only installed if its signature matches the public key built into the app.

### Releases

Pushing a version tag (`v0.5.0`) runs `.github/workflows/release.yml`. It builds and signs the app, and attaches the installer, its signature and `latest.json` (the file installed copies read to find updates) to a **draft** GitHub release. Nothing reaches installed copies until the draft is published on GitHub.

To make a release: set the same version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit, then tag and push.

The signing key is in the repository secret `TAURI_SIGNING_PRIVATE_KEY`; its public half is in `src-tauri/tauri.conf.json`. Keep a backup of the private key. Without it, installed copies can no longer be updated in place.

### Talking to yt-dlp

All yt-dlp commands are built in Rust and passed as argument arrays, never as shell strings. Links are validated as plain `http(s)` URLs and placed after a `--` separator, so user input can't be read as an option.

Two settings keep downloads fast. Segmented streams (HLS, DASH, which most sites other than YouTube use) are fetched four pieces at a time. A trimmed part is fetched by ffmpeg in the request size the site asks for (10 MB on YouTube), because YouTube sends a single large request at only about twice playback speed. The measurements are in [docs/decisions.md](docs/decisions.md#download-speed).

Every download goes through a small HTTP proxy inside Siphon (`src-tauri/src/throttle`), which is how one speed limit covers all of them, including the parts ffmpeg fetches. Each download gets its own listener on `127.0.0.1`, on a port Windows picks, closed when the download ends. HTTPS passes through it as an untouched tunnel. If Windows or the `HTTP(S)_PROXY` variables name a proxy, Siphon's proxy goes through it. Why it works this way, and what was measured, is in [docs/decisions.md](docs/decisions.md#speed-limit).

Progress comes from `--progress-template` lines that the backend parses and folds into one overall percentage, weighted by stream size. Speed and time left are measured by Siphon from the bytes received over the last three seconds, rather than taken from yt-dlp, whose readings jump. It reaches the UI as `download://event` events.

## Project layout

```
src/                     React + TypeScript frontend
  components/            Shared UI (title bar, error card, shadcn/ui primitives)
  features/              One folder per feature: setup, link, video, playlist, presets, options,
                         trim, download, queue, settings, speed, sign-in, home
  lib/                   IPC wrappers, formatting, motion presets
  stores/                Zustand stores (binaries, choices, downloads, trim, toasts, appUpdate, ui, speed, signIn),
                         and the shared settings query
  types.ts               Types mirrored from the Rust backend
src-tauri/               Rust backend (Tauri 2)
  src/binaries/          Download, verify, install and update the managed tools
  src/ytdlp/             Command building, metadata, progress parsing, error classification
  src/plan/              The preset registry, codec rules and the planner (choice → recipe → summary)
  src/choices.rs         The remembered choice and saved presets (choices.json), and migrating old ones
  src/ffmpeg/            Hardware encoder detection and conversion arguments
  src/downloads/         The download queue, jobs (pause, resume, cancel) and progress tracking
  src/throttle/          The speed limit: the local proxy, the shared fair limiter, the system proxy
  src/sessions/          Signed-in sites: cookie jars (DPAPI), which site a link is, the sign-in window,
                         taking a sign-in over from a browser or a cookies.txt file
  src/settings.rs        User preferences and where they are stored
  src/app_update.rs      Checking for and installing a newer Siphon
  src/preview.rs         Where the trim preview plays from (embed, stream or cached copy)
  src/commands.rs        The IPC surface
.github/workflows/ci.yml        Runs the checks on every push and pull request
.github/workflows/release.yml   Runs the checks, then builds and signs a draft release when a version tag is pushed
docs/presets.md          What each preset does and assumes, with sources
docs/sign-in.md          How signing in to sites works, what is kept where, and how to test it
docs/decisions.md        Why things work the way they do, and known issues
app-icon.svg             Icon source; regenerate with `npx tauri icon app-icon.svg`
```

## Stack

Tauri 2, React 19, TypeScript, Vite, Tailwind CSS 4, shadcn/ui, Motion, Zustand, TanStack Query and Lucide icons. There is no telemetry or tracking. The only network requests are the ones needed to fetch a video, to sign in to a site when you ask, and to update Siphon and its managed binaries.

## License

Copyright (C) 2026 Iosif Christogeorgos

Siphon is free software: you can redistribute it and/or modify it under the terms of the [GNU General Public License](LICENSE) as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; see the license for details.

In short: anyone may use, change and share Siphon, but a copy they share, changed or not, must come with its full source under the same license.

yt-dlp, FFmpeg and Deno are not part of Siphon. Siphon downloads them from their own projects when it first starts, and each keeps its own license.
