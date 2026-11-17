# Handoff: smart cut (branch `feat/smart-cut`, not merged)

The owner's brief: replace Exact cut's internals with a smart cut (re-encode only the frames at the cuts, copy the rest), remove Fast cut entirely, validate every result and fall back to the full re-encode, then test live and in players, update the docs and include it in the next batched release.

## Done (committed on this branch)

- **Fast cut removed** (`refactor!` commit): no cut-mode choice in the trim panel; `Trim.exact` and `Choices.exact_cut` gone; an old `exact_cut` in `choices.json` is read past and dropped (test `a_saved_cut_mode_is_dropped`).
- **Smart cut** (`src-tauri/src/ffmpeg/smart_cut.rs`, run by `DownloadManager::smart_cut` in `downloads/mod.rs`): the planner keeps a cut H.264/HEVC stream as a copy and sets `Recipe::cut_encode`. Validation covers frames and timestamps, rising decode times, audio packets at their source times, and the joins decoding (PSNR on the start, an identical first copied frame, a clean decode of the end). Any doubt → `Recipe::with_full_cut()` (the old trim-filter encode). `siphon.log` (`%LOCALAPPDATA%\app.siphon.desktop\logs`, new `src/log.rs`) records the path each cut took, with step timings.
- **The iPhone / iPad / Mac preset re-encodes cuts in full** (planner, `watch.rs`): Apple playback of a stream whose parameter sets change at the joins couldn't be tested. VP9/AV1 are converted when cut, as before (not evaluated further: the AV1/VP9 encoders and players couldn't be made reliable within this work).
- **Parts are fetched 1 s past the end** (`ytdlp/args.rs`, `CUT_MARGIN`), so the last frames decode; the conversion path now cuts the audio too (packet selection, or `atrim` when encoding), and moves audio that is on another clock (AVI parts from archive.org) onto the video's (`smart_cut::clock_shift`, `convert_args_shifted`).
- The summary shows the smart cut's short time beside "No quality loss" (`PlanCard.tsx`).

## Measured (dev build, this PC)

| Case | Before | Now |
| --- | --- | --- |
| Twitch 5 min 1080p60, Max quality | 71.8 s | 10.7 s (cut 3.2 s), smart, checked |
| Twitch 1 h | ~13 min | 81 s (cut 16 s), 216 000/216 000 frames |
| Twitch 15 s, Discord Free | — | 4.5 s, smart (copy fits 20 MB) |
| Twitch 30 s, Apple | — | 9.4 s, full re-encode (by design) |
| YouTube 4K AV1, Max quality | — | 17.7 s, converted (by design) |
| archive.org (MPEG-4 in AVI) | wrong length/audio | 29 s, converted, length fixed |
| YouTube 1080p H.264, Windows | — | **smart cut fails its first-join check, falls back** (24 s, file correct) |

Players, on a prototype clip of the same method: Media Foundation (Media Player's decoder) and VLC decode the joins exactly like the untouched source; ffmpeg verifies copied frames bit-identical.

## Left to do

1. **YouTube 1080p:** the first-join decode check fails with "Invalid data found when processing input" (`head_check_args`). Reproduce with the managed yt-dlp: the section of formats 137+140 is merged into **MKV** (Twitch's is MP4). Suspects: the source's in-band parameter sets/timestamps in MKV, or `-itsoffset` with `-xerror`. Fix or keep the fallback.
2. **Playback checks of app-made files:** browser (WebView2, Siphon's own engine; Edge itself isn't installed), Media Player (`mf-decode.ps1` approach: transcode with `Windows.Media.Transcoding` and compare PSNR against the untouched source as a control), VLC. Premiere and DaVinci Resolve aren't installed; the Edit presets never smart-cut anyway (they always convert).
3. **HEVC smart cut** is untested on real files (tagged `hev1` in MP4). Test with an HEVC source, or fall back for HEVC.
4. **Docs:** README (trimming section still describes Fast cut), docs/presets.md, docs/decisions.md (why Fast cut went, how smart cut + fallback work, the measurements above, the Apple and VP9/AV1 decisions, the AVI clock fix, `-moov_size` instead of `+faststart`: 30 s saved per hour).
5. Re-run `rg -i "fast.?cut|cutmode|cut_mode|keyframe.?cut"` (only the migration test should match), a production build, then merge `--no-ff` into `main` for the next batched release.

Then fold this file into docs/decisions.md and delete it.
