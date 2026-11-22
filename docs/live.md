# Live recording

Paste a live stream (Twitch, YouTube, Kick, or any site yt-dlp records live) and press **Record**. A scheduled stream or an offline channel gets **Record when it starts**. The code is in `src-tauri/src/downloads/live.rs` (recording), `src-tauri/src/downloads/twitch_feed.rs` (Twitch without its ads), `src-tauri/src/plan/live.rs` (planning) and `src-tauri/src/process.rs` (stopping gently).

## What the user sees

- **The card** marks the stream LIVE, UPCOMING or OFFLINE, with how long it has been on air, how many watch, or when it starts. Channel listings show live and scheduled streams as rows that open to record.
- **Presets:** Max quality, Watch and Audio. Share stays in view but disabled, with a hint that it is for the finished recording; Trim is hidden. The plan's size is **per hour**.
- **From:** *Now* (the default) or *The start*, offered only where the site keeps what already aired (a second lookup with `--live-from-start`; YouTube, and Twitch channels that keep VODs). yt-dlp records from the start only on YouTube, Twitch, TVer and mellow-fan; elsewhere it refuses ("no formats that can be downloaded from the start") and *From* isn't shown. Kick is one of those sites: checked 2026-11-21 on a live channel. From the start, what already aired comes at full speed, then it records in real time; the button says "Catching up · 42 min behind live" until it is within 30 s: the stream's age less how long the recording's files are, measured every 5 s (until 0.17, its bytes at the format's stated bitrate, which Twitch states above what a stream averages: it said "44 s behind live" to the end).
- **While recording:** ● REC, the time, the size and the bitrate, and **Stop**. No Pause: a live stream doesn't wait, so a pause would only lose that part. Waiting for a stream shows when it starts, and a way to stop waiting. A Twitch recording read from the stream's VOD (see [Twitch's ads](#twitchs-ads)) records on after Stop to the moment it was pressed: "Catching up to Stop · 8 s", counting down; pressed again, Stop stops it at once.
- **Notes behind the ⓘ:** how much room the disk has for this stream; that recordings go first within the speed limit; on Twitch, that its ads are left out, and what that means. In view: a warning when the speed limit is below what the stream needs, or the disk has room for less than two hours; and a quiet note when three or more recordings are already running.
- **Notifications** (when Siphon isn't in front): the stream went live and recording began; the recording is finished; it stopped with an error.
- **Chat** (Twitch, with Max quality or Watch): the stream's chat drawn into the picture, read as it airs and drawn in as it records, so the file is ready moments after Stop; from the start, with what was said before Record. See [chat.md](chat.md).

## How a recording runs

1. **Waiting.** A stream that isn't on air is looked up again: every 30 minutes while its start is far off, every 30 s around it, every minute for an offline channel or a late stream (`plan::live::next_check`). Siphon does the waiting itself rather than yt-dlp's `--wait-for-video`, so the quality is planned from what the stream really offers once it starts.
2. **Planning.** Each format is given the size of an hour at its bitrate, and the ordinary planner plans it (`plan::live::plan`): the estimate is then per hour. YouTube's live audio states no codec or bitrate; in an MP4 it is AAC, "high" 128 kbit/s.
3. **Recording.** yt-dlp records with `--hls-use-mpegts --no-part --fixup never` into the download folder, as `Title.siphon-<id>.rec1.ts`. MPEG-TS has no index to write at the end, so whatever happens to the process, everything up to that moment plays. From the start, yt-dlp fetches the video and audio side by side (one piece at a time) into two files.
4. **Keeping track.** Each file is written to `downloads.json` (`recordings`) as it appears: from yt-dlp's progress lines, and from the folder itself every 2 s (two streams side by side are reported on stderr). It stays there until the recording is finished.
5. **The end, and dropouts.** When the stream's playlist says it is over and nothing failed, the recording is finished at once (`errors::stream_over`): looking again could still find it "live" for a moment, and a new part would read the ended stream from its start. A stream that stops sending also ends ffmpeg without an error, but leaves "HTTP error 404" and "Failed to reload playlist" in its log (Siphon's own progress lines, which a recording from the start prints there with the file's path, are left out of that reading: before 0.13.1 a title or folder with "error" or "connection" in it made every ended stream look dropped); then the stream is looked at every 20 s for 3 minutes. Live again: recording goes on in `….rec2.ts`. Ended for good (the site says it *was* live): it is finished.
6. **Stop** sends Ctrl+Break to yt-dlp's own process group (`siphon.exe --siphon-interrupt <pid>`, a helper, because only a process on the same console can) and waits for the whole tree. ffmpeg finishes its file ("Exiting normally, received signal 2"); yt-dlp ends at once, so the wait is for ffmpeg. Anything still running after 15 s is killed.
7. **Finishing.** The parts are joined (ffmpeg's concat demuxer; a video and audio pair into Matroska first), copied into the preset's container, or converted for Watch and Audio, cut at the length that has both picture and sound (`-t`; with several parts, each part is ended where its picture and sound both end, with `outpoint` in the concat list, because the demuxer lays each part after the last at its full length: see `ffmpeg::concat_list`), and checked: as long as what was recorded, with the right tracks, sound starting and ending with the picture (lengths from Matroska's DURATION tags where there are no others), and decoding at both ends. Only then are the recorded files removed, with yt-dlp's `-Frag` pieces and `.ytdl` notes.
8. **When it can't be made:** too little room on the disk to copy it, a file that fails its checks, or any other step that fails (joining a recording from the start, the disk), and the recording is kept as it was recorded (`.ts`, or `.mkv` from the start; a video and audio pair that couldn't even be joined, as its two files), with a note on the row. If even that fails, the recording is forgotten and its files left where they are, so it isn't tried again at every start. Until 0.13.0 a failed join or disk error left it in `downloads.json`, failing again at every start, and removing its row didn't clear it (tested 2026-11-20 with a recording from the start whose two files couldn't be read: now kept as its two files at once).

## Twitch's ads

Twitch shows viewers without an account ads in place of the stream: about 30 s as a recording starts, and whenever the streamer runs some. A Twitch recording from now leaves them out, without an account (`downloads/twitch_feed.rs`). Siphon serves the recording a playlist of its own on 127.0.0.1, made from Twitch's, and yt-dlp records that as it would any stream (`--load-info-json`, with the served playlist as the chosen format's stream), so Stop, dropouts, crashes and recovery work as before. Each part (a stream that dropped and came back starts a new one) reads, in this order:

1. **The stream's VOD**, where the channel keeps one: from its newest segment on, as the VOD grows. Twitch saves VODs without those ads, so nothing is missed while one plays, and the recording is on the VOD's clock, so its chat is placed as Twitch's replay places it. The VOD grows ten seconds at a time, a little behind the stream (its newest segment ended about 3 s behind it in one test), so a recording starts with what aired just before Record, and Stop records on until the VOD has what aired when it was pressed (8–15 s in testing, at most three minutes). A VOD that stops growing for a minute is answered "not found", as Twitch answers a dropped stream's playlist, so the recording waits for the stream as for any; its later parts are then read without the ads.
2. **The stream without the ads**, for a channel that keeps no VODs: Twitch's playlist with only the segments it titles `live` (an ad's carry the advertiser's name), numbered as served, with a discontinuity where an ad was left out. Twitch doesn't send what airs during an ad, so the recording jumps over it; ffmpeg closes the jump up, and the file plays on. The first answer waits until the stream itself comes (the ad Twitch opens a playlist with only plays out while the playlist is read, so it is read every 2 s), and lists only the newest segment, so the recording starts with it: when it aired, and where each jump is and when what follows aired, come from what was served, for the chat.
3. **The stream as yt-dlp reads it**, ads and all, if neither can be had (the log says why).

`siphon.log` says which each part reads: `part 1 from the VOD, from its second 23860`, `part 1 without the ads`, or `part 1 with the ads (…)`.

## Crashes, closing and the disk

- **Closing** while recording asks first ("Close and stop"). Recordings are stopped gently, and finished the next time Siphon starts, marked **Recovered**. Waiting ones come back as "Stopped waiting", with **Wait again**.
- **Closing while a recording is being finished** stops the finishing too, and leaves the recording as it was recorded, to be finished the next time Siphon starts (**Recovered**). Until 0.13.0 the stopped finishing counted as a failure: the raw `.ts` was saved under the final name, and the row came back as "Stopped waiting", where **Wait again** started a new recording. Tested 2026-11-20 with a 4K stream converted for Watch: closed 3 s into finishing, the recording was kept and listed in `downloads.json`; at the next start it was made into its MP4 (HEVC, 50 s, as recorded) and the raw file removed.
- **A recording with nothing in it** (Siphon closed in its first seconds) fails at the next start within milliseconds, often before the window has the list of recovered recordings. Such ends are applied again once the list is in place (`recover` in `src/stores/downloads.ts`). Until 0.13.1 the row was marked as being finished afterwards and stayed so, counted as running by the close dialog (reproduced 2026-11-20 with three empty recordings in `downloads.json`: all three stuck on "Finishing 0%"; after the fix all three failed with "Nothing was recorded before Siphon closed.").
- **A crash, or Task Manager:** every process Siphon starts is in a Windows job object that ends them with Siphon, so a recording doesn't go on growing with nobody to stop it. What was recorded is finished at the next start, like after closing.
- **The disk:** a recording stops and is finished before less than 1 GB is free, and says why. The row says how long the disk will last once it is under two hours.

## Speed limit

A recording has to keep up with its stream or it misses parts of it. Its proxy flow reserves its stream's bitrate and a quarter more (`throttle::limiter`): it earns credit at that rate (up to 10 s of it, for segments that arrive in bursts), goes first while it has credit, and everything it gets is paid from it. A reserve below the fair share changes nothing; above it, the recording takes only what it needs, and other downloads share the rest.

Measured 2026-11-19, with a 1080p60 Twitch stream (≈ 0.9 MB/s) and a 4K YouTube download under a 1.5 MB/s limit: the recording got 0.88 MB/s and recorded 89 s of stream in 91 s; the download got 0.66 MB/s. Under 512 KB/s, below the stream's rate, the recording got 58 s of stream in 90 s: it falls behind and parts are missing (it doesn't fail).

## Tested

**Unit tests:** live, scheduled and offline detection; planning per hour, Share refused, from-the-start formats; the speed-limit and disk warnings; the waiting schedule; the recording's readings (bitrate, catching up, disk); the reserved limiter; recording file names; the finishing arguments; the checks on a finished file (lengths, tracks, sync, Matroska tags); cleanup of yt-dlp's companions; the process tree and its kill fallback.

**A fake live stream** (ffmpeg writing a live HLS playlist in real time, a small HTTP server that can refuse everything for a while), in the dev build:

| Case | Result |
| --- | --- |
| Stop after 20 s | MP4 finished 2 s later; raw file removed; plays to the end in ffmpeg, Media Foundation and VLC |
| Stream ends by itself after 60 s | all 60 s recorded, finished at once |
| Server refuses everything for 40 s, then comes back | waited, recorded a second part, joined into one 74 s file that decodes cleanly |
| The same, 2026-11-20, after each part was ended at its own picture and sound | two parts (36.0 s and 40.0 s), joined into a 75.82 s file: exactly what both parts have picture and sound for. Before, the cut came short of the end by each part's mismatch, and the check passed |
| `siphon.exe` killed mid-recording | yt-dlp and ffmpeg ended with it; at the next start, finished as Recovered (28 s; the last frame, cut by the kill, has one damaged block) |
| Closed with "Close and stop" | ffmpeg finished its file (decodes without an error); finished as Recovered at the next start |

**Live** (2026-11-19):

| Case | Result |
| --- | --- |
| Twitch, 1080p, 10 min, Stop | 598 s MP4, 430 MB; plays to the end in Media Foundation and VLC |
| YouTube from now, 1 min, Stop | 70 s MP4, H.264 and AAC copied; decodes cleanly in ffmpeg, Media Foundation and VLC |
| YouTube from the start (a stream 50+ min in), 3 min, Stop | caught up ≈ 4× real time ("55 min behind live"); 12 min 50 s of 1440p60 VP9 + Opus, both tracks equal; decodes cleanly in ffmpeg and Media Foundation, and plays its full length in VLC |
| Scheduled YouTube stream (25 h off) | "Starts in 25 h", waiting state, stop waiting |
| Four offline Twitch channels | recognised as offline, waiting; none went live during the session |
| Speed limit below the stream's bitrate, and shared with a download | see Speed limit |
| Kick, 1 min, Stop (2026-11-21, the owner, installed 0.14.0) | recorded and finished fine; *From* not offered, as yt-dlp can't record Kick from the start |

**Twitch's ads** (2026-11-22, dev build, with chat):

| Case | Result |
| --- | --- |
| From now, a channel with VODs (290 s, Stop over IPC) | Read from the VOD's second 23,860, a few seconds before Record; Stop caught up in 15 s; 290 s of 1080p60, from that second to the segment holding the moment of Stop; no ad |
| The same from the window (another channel, 150 s) | "Catching up to Stop · 8 s", counting down, Stop still there; finished, 150 s |
| From now, a channel without VODs (132 s) | Read without the ads, starting with the first segment served; a frame 103 s in aired exactly when the recording's timing said; 132 s, picture and sound in step |
| An ad left out, simulated: a served playlist with 12 s of a live stream's segments taken out | ffmpeg closed the jump up: picture and sound continuous and together, the segment after it exactly where the lengths of those served before put it |

## Known limitations

- **"Record when it starts" wasn't seen going live:** waiting was tested on a scheduled YouTube stream and offline Twitch channels, but none started during testing. Going live leads into the same path as recording a stream that is already on air.
- **Twitch without VODs: what airs during an ad is missing.** Twitch doesn't send it, so the recording jumps over it. No midroll ad came during testing; the jump was tested by taking segments out of a served playlist (see Tested).
- **Not yet seen in the wild** (2026-11-22): a Twitch stream that drops while it is read from its VOD (the VOD should stop growing, be answered "not found", and the next part be read without the ads), and a recording from the start that reconnects (its next part finds its format by picture size and frame rate). `siphon.log` says what each part read: `part N from the VOD`, `without the ads` or `with the ads (…)`.
- **Twitch with its ads**, where neither the VOD nor the stream without them can be read: as before, at their joins ffmpeg reports a few decode errors (3 in a 10-minute test), and the file plays through.
- **A crash cuts the last frame:** after a kill or power cut, the very end can show a damaged frame. Stop and closing don't.
- **Stopped while catching up**, a recording from the start keeps only the stretch it has both picture and sound for (the sound catches up much faster).
- **Waiting asks the site** every minute or so; many recordings waiting at once may bring on a bot check. Signed-in sites are used as for downloads.
- **Other sites** (beyond Twitch, YouTube and Kick) go through yt-dlp's generic live path and weren't tested.
