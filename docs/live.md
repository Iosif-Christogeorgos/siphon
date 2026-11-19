# Live recording

Paste a live stream (Twitch, YouTube, Kick, or any site yt-dlp records live) and press **Record**. A scheduled stream or an offline channel gets **Record when it starts**. The code is in `src-tauri/src/downloads/live.rs` (recording), `src-tauri/src/plan/live.rs` (planning) and `src-tauri/src/process.rs` (stopping gently).

## What the user sees

- **The card** marks the stream LIVE, UPCOMING or OFFLINE, with how long it has been on air, how many watch, or when it starts. Channel listings show live and scheduled streams as rows that open to record.
- **Presets:** Max quality, Watch and Audio. Share and Edit stay in view but disabled, with a hint that they are for the finished recording; Trim is hidden. The plan's size is **per hour**.
- **From:** *Now* (the default) or *The start*, offered only where the site keeps what already aired (a second lookup with `--live-from-start`; YouTube, and Twitch channels that keep VODs). From the start, what already aired comes at full speed, then it records in real time; the button says "Catching up · 42 min behind live" until it is within 30 s.
- **While recording:** ● REC, the time, the size and the bitrate, and **Stop**. No Pause: a live stream doesn't wait, so a pause would only lose that part. Waiting for a stream shows when it starts, and a way to stop waiting.
- **Notes behind the ⓘ:** how much room the disk has for this stream; that recordings go first within the speed limit; on Twitch, that ads are recorded too (a subscribed or Turbo sign-in can help). In view: a warning when the speed limit is below what the stream needs, or the disk has room for less than two hours; and a quiet note when three or more recordings are already running.
- **Notifications** (when Siphon isn't in front): the stream went live and recording began; the recording is finished; it stopped with an error.

## How a recording runs

1. **Waiting.** A stream that isn't on air is looked up again: every 30 minutes while its start is far off, every 30 s around it, every minute for an offline channel or a late stream (`plan::live::next_check`). Siphon does the waiting itself rather than yt-dlp's `--wait-for-video`, so the quality is planned from what the stream really offers once it starts.
2. **Planning.** Each format is given the size of an hour at its bitrate, and the ordinary planner plans it (`plan::live::plan`): the estimate is then per hour. YouTube's live audio states no codec or bitrate; in an MP4 it is AAC, "high" 128 kbit/s.
3. **Recording.** yt-dlp records with `--hls-use-mpegts --no-part --fixup never` into the download folder, as `Title.siphon-<id>.rec1.ts`. MPEG-TS has no index to write at the end, so whatever happens to the process, everything up to that moment plays. From the start, yt-dlp fetches the video and audio side by side (one piece at a time) into two files.
4. **Keeping track.** Each file is written to `downloads.json` (`recordings`) as it appears: from yt-dlp's progress lines, and from the folder itself every 2 s (two streams side by side are reported on stderr). It stays there until the recording is finished.
5. **Dropouts.** When yt-dlp ends on its own, the stream is looked at every 20 s for 3 minutes. Live again: recording goes on in `….rec2.ts`. Ended for good (the site says it *was* live): it is finished.
6. **Stop** sends Ctrl+Break to yt-dlp's own process group (`siphon.exe --siphon-interrupt <pid>`, a helper, because only a process on the same console can) and waits for the whole tree. ffmpeg finishes its file ("Exiting normally, received signal 2"); yt-dlp ends at once, so the wait is for ffmpeg. Anything still running after 15 s is killed.
7. **Finishing.** The parts are joined (ffmpeg's concat demuxer; a video and audio pair into Matroska first), copied into the preset's container, or converted for Watch and Audio, cut at the length that has both picture and sound (`-t`), and checked: as long as what was recorded, with the right tracks, sound starting and ending with the picture (lengths from Matroska's DURATION tags where there are no others), and decoding at both ends. Only then are the recorded files removed, with yt-dlp's `-Frag` pieces and `.ytdl` notes.
8. **When it can't be made:** too little room on the disk to copy it, or a file that fails its checks, and the recording is kept as it was recorded (`.ts`, or `.mkv` from the start), with a note on the row.

## Crashes, closing and the disk

- **Closing** while recording asks first ("Close and stop"). Recordings are stopped gently, and finished the next time Siphon starts, marked **Recovered**. Waiting ones come back as "Stopped waiting", with **Wait again**.
- **A crash, or Task Manager:** every process Siphon starts is in a Windows job object that ends them with Siphon, so a recording doesn't go on growing with nobody to stop it. What was recorded is finished at the next start, like after closing.
- **The disk:** a recording stops and is finished before less than 1 GB is free, and says why. The row says how long the disk will last once it is under two hours.

## Speed limit

A recording has to keep up with its stream or it misses parts of it. Its proxy flow reserves its stream's bitrate and a quarter more (`throttle::limiter`): it earns credit at that rate (up to 10 s of it, for segments that arrive in bursts), goes first while it has credit, and everything it gets is paid from it. A reserve below the fair share changes nothing; above it, the recording takes only what it needs, and other downloads share the rest.

Measured 2026-11-19, with a 1080p60 Twitch stream (≈ 0.9 MB/s) and a 4K YouTube download under a 1.5 MB/s limit: the recording got 0.88 MB/s and recorded 89 s of stream in 91 s; the download got 0.66 MB/s. Under 512 KB/s, below the stream's rate, the recording got 58 s of stream in 90 s: it falls behind and parts are missing (it doesn't fail).

## Tested

**Unit tests:** live, scheduled and offline detection; planning per hour, Share and Edit refused, from-the-start formats; the speed-limit and disk warnings; the waiting schedule; the recording's readings (bitrate, catching up, disk); the reserved limiter; recording file names; the finishing arguments; the checks on a finished file (lengths, tracks, sync, Matroska tags); cleanup of yt-dlp's companions; the process tree and its kill fallback.

**A fake live stream** (ffmpeg writing a live HLS playlist in real time, a small HTTP server that can refuse everything for a while), in the dev build:

| Case | Result |
| --- | --- |
| Stop after 20 s | MP4 finished 2 s later; raw file removed; plays to the end in ffmpeg, Media Foundation and VLC |
| Stream ends by itself after 60 s | all 60 s recorded, finished 20 s after the end |
| Server refuses everything for 40 s, then comes back | waited, recorded a second part, joined into one 74 s file that decodes cleanly |
| `siphon.exe` killed mid-recording | yt-dlp and ffmpeg ended with it; at the next start, finished as Recovered (28 s; the last frame, cut by the kill, has one damaged block) |
| Closed with "Close and stop" | ffmpeg finished its file (decodes without an error); finished as Recovered at the next start |

**Live** (2026-11-19):

| Case | Result |
| --- | --- |
| Twitch, 1080p, 10 min, Stop | 598 s MP4, 430 MB; plays to the end in Media Foundation and VLC |
| YouTube from now, 1 min, Stop | 70 s MP4, H.264 and AAC copied |
| YouTube from the start (a stream 50+ min in), 3 min, Stop | caught up ≈ 4× real time ("55 min behind live"); 12 min 50 s of 1440p60 VP9 + Opus, both tracks equal; decodes cleanly in ffmpeg and Media Foundation |
| Scheduled YouTube stream (25 h off) | "Starts in 25 h", waiting state, stop waiting |
| Four offline Twitch channels | recognised as offline, waiting; none went live during the session |
| Speed limit below the stream's bitrate, and shared with a download | see Speed limit |

## Known limitations

- **"Record when it starts" wasn't seen going live:** waiting was tested on a scheduled YouTube stream and offline Twitch channels, but none started during testing. Going live leads into the same path as recording a stream that is already on air.
- **Twitch ads** are recorded for viewers without a subscription; Siphon doesn't remove them. At their joins ffmpeg reports a few decode errors (3 in the 10-minute test); the file plays through.
- **A crash cuts the last frame:** after a kill or power cut, the very end can show a damaged frame. Stop and closing don't.
- **Stopped while catching up**, a recording from the start keeps only the stretch it has both picture and sound for (the sound catches up much faster).
- **Waiting asks the site** every minute or so; many recordings waiting at once may bring on a bot check. Signed-in sites are used as for downloads.
- **Kick and other sites** go through yt-dlp's generic live path; only Twitch, YouTube and the fake stream were tested.
