# Subtitles

Pick a language under **Subtitles** and every download that has subtitles in it gets them, in the form its preset needs: a track inside the file, an SRT file beside it for an editor, or drawn into the picture for a chat app. The code is in `src-tauri/src/ytdlp/subtitles.rs` (what a video offers), `src-tauri/src/plan/subtitles.rs` (which track, and where it goes), `src-tauri/src/srt.rs` (cleaning and cutting) and `src-tauri/src/downloads/mod.rs` (fetching and adding).

## What the user sees

- **The choice** sits with the others under the groups: **Subtitles  Off ⌄**. It's a list rather than a facet, because a video can have dozens of languages (a TED talk has 64); typing a language's first letters jumps to it. The list has Off, then each language the video has subtitles in, Windows' language first, then by name. A language that only comes from speech recognition is marked **Automatic**.
- **Siphon remembers the language, not the track.** "Greek" carries over to every video that has Greek. A new user starts on **Off** (the owner's call, 2026-11-20).
- **The plan's line** says what it adds: "No quality loss · English subtitles", "Converts · ~1 min · English subtitles as an SRT file", "Size-optimized · English subtitles in the picture". The ⓘ says where they go and why.
- **A video without the language** downloads without subtitles. The list keeps the language, dimmed, and a note says so and what the video has instead: "No Japanese subtitles for this video. It has English and German." Another language is never put in its place.
- **A video without any subtitles** shows no choice at all.
- **Audio** has no choice: sound files can't hold subtitles. In Customize, an audio-only choice shows it greyed out, with that reason.
- **Playlists** offer the language chosen before, Windows' language and English. Each video uses it if it has it; one that doesn't is saved without, and its row in the download list says **No subtitles**, with the reason in its hint.
- **Live recordings** get none: a stream as it airs has no subtitle file.
- **When subtitles can't be had** (the site refuses, or sends none), the video is still saved, and its row and the finished button say "Saved without English subtitles. The site is limiting requests right now…".

## Where they go

| Preset | Where | File | Why |
| --- | --- | --- | --- |
| Watch: Windows | A track you can turn on and off | **MKV**, with SubRip | Windows' own playback, which Media Player is built on, found the track in an MKV but none in MP4 or WebM, though both played to the end (tested 2026-11-20). Windows plays MKV natively, with the same codecs |
| Watch: TV | A track | **MKV**, with SubRip | Samsung's manuals list SubRip and SubStation Alpha in MKV, and timed text in MP4, among the subtitles their TVs read ([manualslib](https://www.manualslib.com/manual/941619/Samsung-Eb40d.html?page=95)). Other makers' manuals weren't checked |
| Watch: iPhone / iPad / Mac | A track | MP4, with MP4's own timed text (`tx3g`) | Apple's players can't open MKV. Apple's file format names `tx3g` as the subtitle format ([Apple](https://developer.apple.com/documentation/quicktime-file-format/subtitle_sample_description.md)), and QuickTime shows only that ([Apple discussions](https://discussions.apple.com/thread/8591851)) |
| Watch: Android | A track | MP4, with timed text | Android plays MP4 everywhere; not every Android player shows its subtitles (see Known limitations) |
| Max quality | A track | **MKV**, with SubRip, or the original SubStation Alpha with its styling | Holds subtitles as they are, and every player that plays Max quality's streams shows them |
| Share | **Drawn into the picture** | MP4 | Discord, WhatsApp and short-video apps show no subtitle tracks. Can't be turned off |
| Edit | **An SRT file beside the video**: `Title [id].en.srt` | the editor's usual file | Editors import captions as an editable track from a file, and SRT is the caption file they take (Premiere Pro, DaVinci Resolve and CapCut import it; VEGAS wasn't checked) |
| Customize | A track | the file type chosen | MP4 and MOV get timed text, WebM WebVTT, MKV SubRip. For anything but MKV, the plan notes that Windows' own players won't show it |

Every track is tagged with its language in three letters (MP4 dropped a two-letter one: `el` came out with no language, `ell` as Greek; Matroska takes the bibliographic form, `gre`), named ("English"), and marked to show when the video opens, since it was asked for. In MP4 and MOV it is tagged `tx3g` with Apple's `sbtl` handler: ffmpeg does so for MP4 by itself, but tags MOV's `text`, which QuickTime ignores ([FFmpeg ticket 1845](https://trac.ffmpeg.org/ticket/1845)), unless told.

## How a download gets them

1. **What the video offers.** `yt-dlp -J` lists subtitles by language in two places: `subtitles` (written by people) and `automatic_captions` (made by machine). YouTube's machine-made ones are speech recognition in the language spoken (key `en-orig`, "English (Original)"), copies of the people's tracks (as HLS), and machine translations into about 155 languages. Siphon keeps the people's tracks and the speech recognition, and leaves out chat replays (`live_chat`, `rechat`, Bilibili's `danmaku`), tracks only in formats nothing here reads (YouTube's `json3` and `srv`), and **the machine translations**: YouTube refused every one asked for in testing, four of four on two videos, spaced 20 s apart, each with HTTP 429.
2. **Which track.** People's beat speech recognition whatever the region (`en-GB` written by people over `en-US` by machine); then the exact region; then the plain language (`en` over `en-gb`). Languages are compared however a site writes them: `pt_BR`, `pt-BR` and `pt-br` are one, and so are `ger` and `de`.
3. **Fetching.** Its own yt-dlp run beside the video's (`--skip-download`), so subtitles that can't be had never cost the video. It goes through Siphon's proxy and the site's sign-in like the download, and starts from the same saved lookup (see decisions.md, "Download speed"), so it doesn't look the video up again; a download without a saved lookup looks it up, about 3 s on YouTube and 2 s on TED. The video's progress shows a subtitles step only if they are still on their way when the video is done. SubRip is asked for first: YouTube's WebVTT for speech recognition repeats every line as it rolls up, with a timing tag on every word, and comes out as junk when converted, while its SubRip doesn't. Anything else is made into SubRip by yt-dlp (it reads TTML itself, WebVTT through ffmpeg). A refusal that may pass is tried once more after 5 s. A language yt-dlp turns out not to have ends with no file and exit code 0, so the file is what's checked.
4. **Preparing** (`srt.rs`). The file is read leniently, empty cues are dropped, and a clip keeps only what shows inside it. Cues that overlap are split where any starts or ends, each piece showing every line up then: YouTube's speech recognition keeps each line up while the next arrives, two on screen, rolling up, and an MP4 track can't hold cues that overlap. Measured on a 30 s clip of a Blender overview: 14 cues, none overlapping, reading two lines at a time as on YouTube.
5. **Adding.** A track goes in during Siphon's ffmpeg pass, which a download with subtitles always has, even when its streams are only copied (one more write of the file, at disk speed). For a clip, the subtitles stay on the video's own clock and are shifted with the picture. A smart cut is joined and checked as MP4, with the track in it, then copied into MKV when the plan asks for it: its checks read decode times, which Matroska doesn't keep, and every smart cut into MKV failed them and was re-encoded in full. An SRT for Edit is written beside the finished file, numbered if the name is taken. For Share, ffmpeg's `subtitles` filter (libass) draws them during the size-targeted encode, before scaling, with Windows' fonts through DirectWrite.

## Tested

2026-11-20, in the dev build, driven over the DevTools protocol; the files checked with ffprobe and Windows' own playback API (`Windows.Media.Playback`, from PowerShell).

| Case | Result |
| --- | --- |
| *Me at the zoo*, Watch for Windows, English | MKV with a SubRip track, tagged `eng`, named English, on by default; Windows' playback lists "English" |
| The same in MP4 and WebM (before Windows got MKV) | Both played to the end in Windows' playback; it found no subtitle track in either |
| Max quality, German | AV1 + AAC in MKV with a German track (`ger`) |
| Edit for Premiere, German | `Me at the zoo [jNQXAC9IVRw].de.srt` beside the video, umlauts intact |
| A clip (0:30–0:45) smart-cut for Windows, English | 302 of 375 frames copied, checked, 1.5 s; MKV with a 15.000 s English track starting with the line sung at 0:30; Windows played it to 14.7 of 15 s, track listed |
| A clip cut during a conversion (0:04–0:12) | First cue moved from 5.3 s to 1.3 s, the last cut off at 8.000 s |
| Custom MP4 with H.264 converted, WebM, MKV | timed text, WebVTT (`eng`), SubRip (`ger`) |
| Speech recognition (a Blender overview), a 30 s clip for Windows and a 15 s clip for CapCut | 14 cues in the track, none overlapping; the SRT beside the CapCut clip starts at 0 |
| A machine translation (Greek, before they were left out) | Refused twice with HTTP 429; the video saved, with "Saved without Greek subtitles. The site is limiting requests right now…" |
| A language the video lacks (French) | Saved without, with "This video has no French subtitles." |
| Share for Discord, English, the whole video and a clip | Subtitles in the picture at the right moment (a frame at 0:06 shows the line said from 0:05.3; the clip's frame at 0:02 shows the line sung from 0:31.1); the 1080p clip came out at 19.7 MB, under 20 MB |
| Drawing Greek, Japanese and Arabic | Arial for Greek; Japanese fell back to Yu Gothic UI; Arabic joined and right to left |
| The window | The choice, the list for TED's 64 languages (typing "gr" jumped to Greek), the plan's line and ⓘ for each group, the note for a missing language, the playlist list, Audio hiding the choice |

**Unit tests:** reading YouTube's, TED's and other shapes; leaving out chat, unreadable formats and translations; language tags and names; picking a track; where each preset puts it and in which file type; fetching arguments (never a pattern, "all", or a language left out); reading, writing, cutting and flattening SubRip; the ffmpeg arguments for a track and for drawing one in; the name beside the video.

## Known limitations

- **No machine translations.** YouTube refuses them (see above). Speech recognition in the language spoken, and people's subtitles, come through.
- **Apple, Android and TVs weren't tested on a device.** Apple's rules follow its file format; Android's built-in players vary in whether they show an MP4 subtitle track (VLC and players built on Google's ExoPlayer do); TVs follow their makers' manuals.
- **Styled subtitles (SubStation Alpha) weren't tested end to end**: no site at hand offered them. The path is the same as for SubRip, without the conversion.
- **A language Windows has no name for** shows its code in the list ("inh" on TED).
- **One more yt-dlp run** per download with subtitles, beside the video's; from the saved lookup it asks the site only for the subtitle file.
