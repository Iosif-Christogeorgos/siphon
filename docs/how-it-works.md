# How Siphon works

What each part of Siphon does and why, in more depth than the [README](../README.md). The reasons behind the harder choices, with what was measured, are in [decisions.md](decisions.md); what each preset assumes, with sources, is in [presets.md](presets.md); signing in to sites is in [sign-in.md](sign-in.md); subtitles are in [subtitles.md](subtitles.md); Twitch chat is in [chat.md](chat.md); converting files on this PC is in [local-files.md](local-files.md). Building and releasing are in [development.md](development.md).

## Managed binaries

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

## Presets and the planner

Presets start from where the video is going. Pick a **group**, then a **target** in it, and optionally a **quality**:

| Group | Targets | What you get |
| --- | --- | --- |
| Max quality | | The best streams exactly as the site has them; may not play everywhere |
| Watch | Windows · iPhone / iPad / Mac · Android · TV | The best version that plays natively on the device |
| Audio | Music · Best audio (original) | MP3 or M4A at a chosen bitrate, or the original track untouched |
| Share | Discord · WhatsApp · TikTok / Reels / Shorts | The best quality that fits the upload limit of your tier |

The **Quality** choice offers **Best** and the resolutions the video actually has. It shows only the current pick until you point at it; it then unfolds across the row, and each resolution shows its estimated size as you point at it (a range when converting, since the size then depends on the picture); a lower one downloads the matching stream, and nothing is upscaled. Music offers bitrates instead; Max quality and Share have no quality row. **Customize** (under the summary) opens the old manual mode, where you choose the streams, file type and codecs yourself, and can save them as a named preset that appears under **My presets**. Siphon remembers the last group, target, quality and Share tier.

The presets are defined once, in `src-tauri/src/plan/registry.rs`, and the interface reads them from the backend. The planner in `src-tauri/src/plan` turns a choice into a recipe from the formats the video really offers: the same Apple preset copies X's 4K H.264 file but converts YouTube's 4K VP9 to HEVC. It copies original streams whenever the target can play them, because converting is slower and never improves quality. The summary above the Download button is generated from the same recipe that builds the commands, so the app cannot describe one thing and do another. It marks each plan **No quality loss**, **Converts** (with a rough time), **Size-optimized** or **May not play everywhere**.

**Sizes.** The summary's size is near-exact for anything copied as the site states it, a Share file, or audio at a set bitrate. A conversion's depends on the picture, so it starts as a range from a rule of thumb, and a trimmed part's from its share of the whole. Meanwhile Siphon measures them on the video itself, in the background: the part's bytes, read from the stream's index; the frames a smart cut re-encodes; a dozen 4-second stretches encoded exactly as the download will encode them. Within seconds (about 20 for a 4K conversion), the range becomes one figure, within about a tenth. Nothing waits for it: Download starts at once and ends it. How, and what was measured, is in [decisions.md](decisions.md#size-estimates).

When a conversion is needed, Siphon runs its own ffmpeg pass with a hardware encoder (NVENC, Quick Sync or AMF) if a short test encode shows it works on your machine, and falls back to software otherwise. The downloaded source waits in a work folder until the finished file is written and checked, so a failed conversion can be tried again without downloading again.

**Share** works out the bitrate from the limit and the length (the trimmed length, if you trimmed), picks the size and frame rate that look best for it (a clean 720p30 rather than a blocky 1080p60), and checks the finished file, encoding again a little smaller if it came out over. An original that already fits is copied as it is. A video too long to look decent under the limit isn't made at all: Siphon suggests trimming it instead.

What each target plays or needs, the upload limits, and the sources for them are in [presets.md](presets.md).

## Trimming

**Trim** on the video card opens a preview player with a timeline. Drag the two handles, type the times, or use the keyboard, and add more parts if you want several clips. Each part is downloaded as its own file.

| Key | Action |
| --- | --- |
| Space | Play or pause (playback stops at the end of the part) |
| ← / → | Move one second (with Shift: five) |
| , / . | Step one frame back or forward |
| I / O | Set the start or end of the part at the playhead |

The preview comes from YouTube's embedded player for YouTube links, from a low-resolution stream for other sites (a plain file if the site has one, otherwise HLS, which is all Twitch offers), and from a small low-resolution copy if neither plays. A file on this PC plays as it is where WebView2 can, and from a small copy made of it otherwise.

Only the selected part is downloaded (`--download-sections`), and every clip starts and ends on the exact frames chosen. There is no choice to make:

- **When the stream is kept as it is** (H.264 or HEVC), Siphon makes a **smart cut**: it re-encodes only the frames from the start of the clip to the next key frame and from the last key frame to the end, about two seconds each, and copies everything in between untouched. The re-encoded frames match the original's settings and are visually lossless. An hour of 1080p60 from Twitch is cut in about 20 seconds.
- **When the video is converted anyway** (Share at a lower size, VP9 or AV1 kept as they are, or a clip for iPhone / iPad / Mac), the cut is made during that conversion.

Every smart cut is checked before it is kept: every frame at its time, the sound in sync from start to end, and the joins decoding exactly like the original. If anything is off, the whole clip is re-encoded instead, so a clip never comes out broken. `siphon.log` (in `%LOCALAPPDATA%\app.siphon.desktop\logs`) records which way each cut was made. How it works, and what was measured, is in [decisions.md](decisions.md#trimming).

## Subtitles

**Subtitles** beside the other choices picks a language (or Off, where a new user starts), and Siphon remembers the language, so every video that has it gets it. The list has the video's languages, Windows' language first; typing jumps to one. People's subtitles come before speech recognition; YouTube's machine translations aren't offered, since YouTube refuses them. A video without the language is saved without subtitles and says so, never with another language in its place.

Where they go depends on the preset: a track inside the file for Watch and Max quality (MKV for Windows, TVs and Max quality, because Windows' own players don't show subtitle tracks in MP4 or WebM; MP4 timed text for Apple and Android), and drawn into the picture for Share, since chat apps show no tracks. The track is fetched by a yt-dlp run of its own beside the video's, so subtitles that can't be had never cost the video; a clip gets only the part inside it. How, why, and what was tested is in [subtitles.md](subtitles.md).

## Chat

For a Twitch video, a Twitch channel's videos or a Twitch stream, with Max quality or Watch, **Chat** (*With* or *Without*) draws the chat into the picture, the way it looked on Twitch. **Chat** on the video's card opens the preview with the chat box over it, showing the real chat at that moment (for a stream on air, a still of the stream with the chat as it comes): drag it, resize it, set how dark it is; the place is kept for the channel. A video's chat comes from Twitch's replay, fetched beside the video; a recording's is read as the stream airs, placed in the file where Twitch's own replay shows it, and drawn in as it records, so the file is ready moments after Stop. How it is read, timed and drawn, what was measured, and its limits are in [chat.md](chat.md).

## Files on this PC

Video and sound files dropped on the window, or picked with **Browse** beside Paste, are converted the way a download is: planned by the same presets, made by Siphon's own ffmpeg pass straight from the file, and saved beside it (`Clip (Windows).mp4`, `Clip (Discord, 1m00s-1m15s).mp4`, `Clip.mp3`), never under a name in use; a folder that can't be written to sends it to the download folder. The file itself is only ever read. Max quality isn't offered for a file, which already is its own best. Watch and Customize keep a file's other sound tracks and its subtitle tracks, where the file type holds them; Share and Audio take its main sound alone. A file that already is what was asked for says so, and nothing is made. A folder gives the video and sound files directly inside it; several files open a list to choose from, with one preset for all. Conversions take turns by what they need (one encode on the processor, two on the graphics card) and run below other programs' priority. The webview never names a file: the backend takes the paths from the window's drop event or its own file dialog, and gives the webview a key for each. How, what was measured and the limits are in [local-files.md](local-files.md).

## Queue, playlists and settings

Every download joins a queue. A set number run at once (three by default) and the rest wait their turn, in order. The **Activity** button in the title bar opens the list, which holds conversions of files too, and where each can be paused, resumed, cancelled, retried, opened or shown in its folder.

- **Pause** stops the download but keeps what it has fetched; **Resume** carries on from there.
- While Siphon converts a video, its row and the bar under Download say the size the file is heading for ("≈ 103 MB"), worked out from what it has written so far.
- **Cancel** stops it and deletes the partial files.
- A download that fails because the connection dropped or the site refused it is tried again on its own: twice, after 2 and 10 seconds. A site that is limiting requests (HTTP 429) gets one more try, after 30 seconds. The download shows "Trying again in a moment" while it waits.
- Every new try, automatic or with **Try again**, carries on from what the last one fetched instead of starting over.
- A trimmed part whose connection stops sending gives up after 20 seconds and is tried again, instead of waiting forever.
- Downloading something again never replaces or touches a file already in the folder: the new one is named `Title [id] (2).mp4`, then `(3)`, and so on. While a download runs, its files carry a `.siphon-…` marker in their name, which comes off when it is finished.
- Opening a video you have downloaded before says so under its card ("You already have this · today · 860 MB", or "a clip of this"), with **Open** and **Show in folder**, as long as the file is still where Siphon put it. A video that is downloading or waiting from elsewhere (a playlist, say) shows "Already in your downloads".
- When downloads or conversions finish or fail while Siphon isn't the active window, Windows shows a notification. Those that end within two seconds of each other share one ("3 downloads finished", "Conversion failed").
- Closing the app pauses every download, so no yt-dlp or ffmpeg process is left behind; while any are running or waiting, Siphon asks first. The list is kept between sessions: paused downloads come back paused and carry on from where they stopped (**Resume all** resumes them together), and finished ones can still be opened.

**Speed limit.** Settings → Downloads → **Speed limit** caps the total download speed of all downloads together: Unlimited (the default), 20, 30, 40 or 50 MB/s, or a custom value. Running downloads share it evenly, a download that needs less leaves the rest to the others, and a change applies to running downloads within a second. The limit is kept in `settings.json`, so it stays on until it is changed. While it is on, the title bar says so ("Limited to 20 MB/s"; click it to change it), and while anything downloads it shows the total speed. MB here is 1,048,576 bytes, the unit download speeds are shown in; a figure moves to the next unit at 1,000 of one, so it never reads "1023 KB/s". How it works is under [Talking to yt-dlp](#talking-to-yt-dlp).

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

## Signing in

When a video needs an account (age-restricted, members-only, a post only signed-in viewers see), the error says which site ("**youtube.com** needs you to sign in") and offers **Sign in**, with the way to sign in beside it:

- **Sign-in window**: the site's own sign-in page in a Siphon window, with its own browser profile. Sign in as usual and press **Done**; on YouTube, Instagram, X and a few others it finishes by itself.
- **A browser on this computer**: takes the sign-in over from it. Firefox works; Chrome, Edge and Brave on Windows encrypt their cookies so only they can read them, and Siphon says so.
- **cookies.txt**: a cookies file exported from your browser with an extension.

Every video is tried without an account first, even on a site you're signed in to, so you're never asked for one a video doesn't need. When a site refuses, Siphon says how likely an account is to help and lets you choose: a login wall or a members-only video opens the sign-in offer (with **No thanks**), the bot check and unrecognised errors get a quiet link with the risk explained, and DRM, removed or region-blocked videos get none. Once you're signed in to a site, a video that needs it uses your sign-in by itself, and its card says **Signed in**.

Siphon checks the sign-in on the video before saying it worked, then looks the video up again and retries that site's failed downloads. Each site's cookies are kept apart, encrypted for your Windows account, and given to that site's downloads only, in a temporary file deleted when the download ends. A sign-in that has run out says so, and so does an account without access. **Settings → Signed-in sites** lists them, with **Sign out** and when to use each: **When needed** (the default) or **Always**. Details, including why Google sometimes refuses embedded sign-in windows, are in [sign-in.md](sign-in.md).

## Live recording

A live stream gets **Record** instead of Download, and a scheduled stream or an offline channel **Record when it starts**. Siphon waits for it, records it as MPEG-TS (so nothing recorded is lost to a crash), carries on through short dropouts, and when it ends or is stopped makes it into the preset's file (Max quality, Watch or Audio) and checks it. Where the site keeps what already aired, it can record **from the start**, catching up first. On Twitch the ads are left out: a recording reads the stream's VOD where the channel keeps one, and otherwise the stream with the ads taken out of its playlist (Siphon serves the playlist yt-dlp records). Recordings go first within the speed limit and are finished after a crash or closing. Details, measurements and limits are in [live.md](live.md).

## Updates

Video sites change often and yt-dlp follows within days, so an old yt-dlp is the usual reason a download stops working. Siphon keeps its tools current on its own.

- **When.** The tools are checked at launch and once a day. **Check now** in Settings → Updates does it on demand.
- **Channel.** yt-dlp comes from its stable releases by default. **Nightly** switches to `yt-dlp/yt-dlp-nightly-builds`, which gets site fixes sooner.
- **Install automatically or ask first.** By default a new version is installed in the background and a toast says what changed. With **Ask first**, the toast offers the update and nothing is replaced until you accept. A tool that is missing is always installed, because Siphon can't work without it.
- **Never mid-download.** A tool is not replaced while a download is using it. The update waits and is tried again within the hour.
- **Update and retry.** If a download fails in a way that points to an outdated yt-dlp (a bot check, an unreadable page, HTTP 403 or 429), Siphon looks for a newer yt-dlp and, if there is one, installs it and tries the download once more before reporting the error. If there is none, a refused or limited request is still tried again after a pause (see [Queue](#queue-playlists-and-settings)), and the video is looked up afresh in case the site changed what it offers.

Siphon also updates itself. At launch it looks for a newer published release on GitHub. If there is one, a toast and Settings → Updates offer **Restart to update**. Siphon always asks first, because it has to restart, and it refuses while downloads are running or waiting. The update is only installed if its signature matches the public key built into the app.

## Talking to yt-dlp

All yt-dlp commands are built in Rust and passed as argument arrays, never as shell strings. Links are validated as plain `http(s)` URLs and placed after a `--` separator, so user input can't be read as an option.

A download starts from what yt-dlp printed when the link was pasted (`--load-info-json`), rather than looking the video up again, if that was less than 15 minutes ago: about 2 s sooner on YouTube. Two settings keep downloads fast. Segmented streams (HLS, DASH, which most sites other than YouTube use) are fetched four pieces at a time, and so are YouTube's streams, which yt-dlp is told to fetch as 10 MB pieces (`youtube:formats=dashy`) rather than on a single connection, whose speed varies. A trimmed part is fetched by ffmpeg in the request size the site asks for (10 MB on YouTube), because YouTube sends a single large request at only about twice playback speed. The measurements are in [decisions.md](decisions.md#download-speed).

Every download goes through a small HTTP proxy inside Siphon (`src-tauri/src/throttle`), which is how one speed limit covers all of them, including the parts ffmpeg fetches. Each download gets its own listener on `127.0.0.1`, on a port Windows picks, closed when the download ends. HTTPS passes through it as an untouched tunnel. If Windows or the `HTTP(S)_PROXY` and `ALL_PROXY` variables name a proxy, HTTP or SOCKS, Siphon's proxy goes through it, and so do the update checks. Why it works this way, and what was measured, is in [decisions.md](decisions.md#speed-limit).

Progress comes from `--progress-template` lines that the backend parses and folds into one overall percentage, weighted by stream size. Speed and time left are measured by Siphon from the bytes received over the last three seconds, rather than taken from yt-dlp, whose readings jump. It reaches the UI as `download://event` events.

## Project layout

```
src/                     React + TypeScript frontend
  components/            Shared UI (title bar, error card, shadcn/ui primitives)
  features/              One folder per feature: setup, link, video, playlist, presets, options,
                         trim, download, queue, settings, speed, sign-in, home, chat, files
  lib/                   IPC wrappers, formatting, motion presets
  stores/                Zustand stores (binaries, choices, downloads, trim, toasts, appUpdate, ui, speed, signIn),
                         and the shared settings query
  types.ts               Types mirrored from the Rust backend
src-tauri/               Rust backend (Tauri 2)
  src/binaries/          Download, verify, install and update the managed tools
  src/ytdlp/             Command building, metadata, the subtitles a video offers, progress parsing, error classification
  src/plan/              The preset registry, codec rules and the planner (choice → recipe → summary), where subtitles go,
                         and the sizes measured on a video (sizes.rs)
  src/sizing/            Measuring a download's size in the background: a part's bytes, a smart cut's ends, a conversion's stretches
  src/srt.rs             Reading, cleaning, cutting and writing SubRip subtitles
  src/choices.rs         The remembered choice and saved presets (choices.json), and migrating old ones
  src/ffmpeg/            Hardware encoder detection and conversion arguments
  src/downloads/         The download queue, jobs (pause, resume, cancel), progress tracking, and live recording (live.rs; Twitch without its ads: twitch_feed.rs), drawing chat in (chat.rs, live_chat.rs, live_draw.rs), and converting files on this PC (files.rs)
  src/local/             Files on this PC: taking them in (drops, Browse, folders), the keys the webview knows them by, reading them with ffprobe, and what is made of them and called
  src/chat/              Twitch chat: reading it (replay, IRC), emotes and badges, and drawing it as a layer
  src/throttle/          The speed limit: the local proxy, the shared fair limiter, the system proxy
  src/sessions/          Signed-in sites: cookie jars (DPAPI), which site a link is, the sign-in window,
                         taking a sign-in over from a browser or a cookies.txt file
  src/settings.rs        User preferences and where they are stored
  src/app_update.rs      Checking for and installing a newer Siphon
  src/preview.rs         Where the trim preview plays from (embed, stream or cached copy)
  src/commands.rs        The IPC surface
.github/workflows/ci.yml        Runs the checks on every push and pull request
.github/workflows/release.yml   Runs the checks, then builds and signs a draft release when a version tag is pushed
docs/how-it-works.md     This file
docs/development.md      Building, checking and releasing
docs/presets.md          What each preset does and assumes, with sources
docs/sign-in.md          How signing in to sites works, what is kept where, and how to test it
docs/live.md             Recording live streams: how, what was tested, and its limits
docs/subtitles.md        Subtitles: which a download gets, where each preset puts them, what was tested
docs/chat.md             Twitch chat drawn in: how it is read, timed and drawn, what was measured
docs/local-files.md      Converting files on this PC: how, what was tested, and its limits
docs/decisions.md        Why things work the way they do, and known issues
docs/media/              The README's screenshots and demo
app-icon.svg             Icon source; regenerate with `npx tauri icon app-icon.svg`
```

## Stack

Tauri 2, React 19, TypeScript, Vite, Tailwind CSS 4, shadcn/ui, Motion, Zustand, TanStack Query and Lucide icons. There is no telemetry or tracking. The only network requests are the ones needed to fetch a video, to sign in to a site when you ask, and to update Siphon and its managed binaries.

