# Files on this PC

Drop video or sound files on the window, or pick them with **Browse**, and Siphon converts them the way it would a download: for a device, to fit a chat app's limit, into another file type, their sound alone, or a part cut out on the exact frames. What it makes goes beside each file; the file itself is never touched. The code is in `src-tauri/src/local/` (taking files in, reading them, naming what is made), `src-tauri/src/downloads/files.rs` (making it), and `src/features/files/` with the start screen (`src/features/home/Home.tsx`, `src/features/link/LinkInput.tsx`).

## What the user sees

- **Taking files in.** **Browse** beside Paste opens Windows' file dialog; files dragged over the window bring up a target over all of it, with the focused link field's aqua edge and glow, saying *Drop to convert*. A folder gives the video and sound files directly inside it, in Explorer's order (Clip 2 before Clip 10), not its subfolders; anything else in it is counted and left out ("2 not video or sound, left out"). At most 500 files at once. A file dropped on its own is taken whatever it is called, and reading it says whether it is video or sound.
- **One file** shows in the field by its name, and in a card like a video's: a frame a tenth of the way in (or an audio file's cover, or a sound icon), its folder and size, its resolution and what it holds ("1080p60", "H.264 + AAC"). The presets plan it as for a link, all but **Max quality**, which picks the best a site has: a file already is its own, and opens on Watch if Max quality was chosen. The button says what it makes: *Change the file type* (when nothing is converted, only moved into another file type), *Convert*, *Make it fit* (Share), *Take the audio*, *Cut the clip*.
- **Nothing to do.** A file that already is what was asked for (an H.264 MP4 for Windows, an MP3 for Music) says so, "This already plays on Windows 11.", with what it is underneath, and the button rests on *Nothing to convert*. No job starts and no file is made.
- **Several files** open a list like a playlist's: each file read (three at a time) and planned with the one choice as the list fills in, rows rising in; checkboxes, shift-click, *Find by name* from ten files, and the total size. A file with nothing to do is marked with why ("Already plays on Windows 11") and left out; one that can't be read is marked *Can't be read*. One button converts them all ("Convert 13 files").
- **Trim** opens the same trim panel as for a link. The preview plays the file itself where WebView2 can, and otherwise a small copy made of it (*Making a small copy to preview…*).
- **Activity.** The list of downloads is called Activity, since it holds conversions too. A conversion is a row like a download's, with *Converting* and the size it is heading for, then *Open* and *Show in folder*. Notifications say *Conversion finished*; closing while files convert says they pause and start over.

## Where what is made goes

Beside the file, named after it with what it was made for and the part cut out, if any (`local::output_name`):

| Made | Name |
| --- | --- |
| Watch | `Clip (Windows).mp4`, `Clip (iPhone).mp4`, `Clip (Android).mp4`, `Clip (TV).mp4` |
| Share | `Clip (Discord).mp4`, `Clip (WhatsApp).mp4`, `Clip (TikTok).mp4` (or Reels, Shorts) |
| A part | `Clip (Discord, 1m00s-1m15s).mp4`, `Clip (1m00s-1m15s).mp4` |
| Sound, Customize | `Clip.mp3`, `Clip.m4a`, `recording.mp4`: the file type says it |

A name already taken gets `(2)`, `(3)`, … as downloads do, so nothing is ever overwritten; until it is finished and checked, the file is written under the job's own marker (`Clip (Windows).siphon-1a2b3c4d.mp4`). A folder Siphon can't write to (a disc, a read-only share, a folder Windows guards against unknown apps) sends it to the download folder instead, with a note: "Saved in your download folder, because the file's own folder can't be written to."

## How it works

**The webview never names a file.** The paths come to the backend from the window's own drop event, which carries them, or from the file dialog the backend opens itself (`pick_files`). It keeps them (`local::Grants`) and gives the webview a key for each, `siphon-file:` and the first 16 hex digits of the SHA-256 of the file's path, which stands where a link would: `read_file`, `plan_download`, `start_download` and the preview all take it. A key nobody was given, or a path in its place, is refused. A path typed or pasted into the field is never taken.

**The original is only ever read.** It is an `Original`, which gives its path for reading and nothing else; the steps that move, rename or delete a file (`move_file`, `publish`, `check_section`) take a `Made` file, one Siphon downloaded or recorded, and an original never becomes one. What is made is written next to it under the job's marker and renamed once checked; the job's work folder (a two-pass log, a smart cut's pieces) is Siphon's own.

**Reading** (`local::read`, `local/probe.rs`): ffprobe, at most three at a time, into the shape a link's lookup has: one format with the file's own picture and sound, so the planner plans it like any other. Its first video track that isn't a cover picture, its default sound track (`0:V:0` in ffmpeg's arguments skips covers too); a phone video stored on its side reads upright; an audio file's cover art is a picture, not video; a still picture or anything without a length is refused. What was read is kept until the file changes: each file carries a version, its size and when it last changed, which the window keeps its readings and plans by, and planning reads the file through the same check rather than a cache, so a file changed since is never planned from what it used to hold.

**Making** (`downloads/files.rs`): the recipe is made by Siphon's own ffmpeg pass straight from the file, by the steps a download's conversion takes (`convert_checked`: smart cut, conversion, the hardware encoder's fallback to the CPU, Share's check against its limit, the check that it holds a picture or sound); sound alone by `ffmpeg::sound_args`, with its tags and its cover where the file type holds one (MP3, M4A, FLAC). A copy for Share that comes out over the limit after all is converted. Before it starts, a conversion checks its disk has room for what it will make, a fifth more and a quarter of a gigabyte. Nothing to do is said by `local::nothing_to_do`: the recipe copies every track untouched, keeps picture and sound alike, and makes the same kind of file; a sound file is its own original sound whatever it is in, and only Music, which asks for one, makes another of it.

**Turns.** Conversions of files wait in a queue of their own, not the downloads' (`FilesGate`), by what they take: one encode on the processor at a time, two on the graphics card, four of sound alone, two copies. What they run is below other programs' priority (`process::below_others`), so the PC stays usable through a batch.

**A paused or failed conversion** remembers its file in `downloads.json` (`originals`), and resumes on it after a restart, when the window's key would otherwise be unknown; finished or cancelled, it is forgotten. A conversion has nothing to carry on from: resumed, it starts over.

**A part of a file** is chosen from the file's start (as its preview shows it) and cut by its timestamps, so an OBS recording whose timestamps start seconds in is cut where it should be. A whole file is read from half a minute before the part rather than from its start (`plan::ReadFrom`): every read of it, by the smart cut and by a conversion, seeks there and stops a few seconds past the part, and the end's packets stop there too. The frames are still picked by their own timestamps, so a key frame landed on early costs nothing. Downloads, whose parts start near the cut anyway, read as before.

**The preview** (`preview::file_source`) plays the file itself where WebView2 plays it (MP4, MOV or WebM with H.264, VP9, VP8 or AV1, and common sound files), let through the asset protocol on its own; its scope is otherwise only Siphon's folders. Anything else (Matroska, HEVC, MPEG-TS) plays from a 360p copy made here, at below-normal priority, with a key frame every second; its timestamps start at zero where the file starts, as a part is counted. Cards' frames are kept in `<app cache>/frames`, by file id, and let go after a month.

## Tested

2026-11-23 to 25, in the dev build, driven over the DevTools protocol; each file's SHA-256 and modified time checked before and after.

| Case | Result |
| --- | --- |
| Every kind of job: a container change, MPEG-TS to MP4, VP9 to HEVC for Apple, Share, two smart cuts, three of sound, the read-only folder; then paused and resumed, cancelled, failed, and a processor encode paused, resumed and cancelled | Every original kept its SHA-256 and modified time; nothing left beside them but what was made |
| Nothing to do (an H.264 MP4 for Windows) | No file; started over IPC anyway, it failed at once with the reason |
| A part of an MPEG-TS whose timestamps start at 2.78 s | 180 frames, from the file's own second 2 |
| A file in `C:\Windows\Media` (Alarm01.wav) to MP3 | Saved in the download folder, with the note |
| A name with Unicode, brackets, `%` and `#` | Read, planned and converted |
| A folder of 15 files, a document and an image beside them, a subfolder | 15 listed, 2 left out, the subfolder not looked in; the MP3 marked as already MP3, the renamed text file as unreadable |
| 50 clips of 20 s 1080p60, 70 MB each, for Discord | All 50 in about a minute; never more than two ffmpeg at once, all below normal priority; the GPU's encoder at 80% on average, the processor 42%, Siphon's own memory 57–59 MB |
| A paused conversion, Siphon restarted, then resumed | Resumed on the file it was started on |
| The same 50 names dropped again after the files changed | Read and planned again (found by this test: they had been planned as the old files) |
| 18:00–18:10 of a 20-minute 1080p30 MKV | Smart-cut in 1.1 s; re-encoded in full for Apple in 2.2 s; both from source frame 32400 exactly |
| A 10.5 GB, 50-minute file | Its card within seconds of Browse; 45:10–45:15 cut in 3.4 s, 300 frames; the original unchanged |
| A real drop (an OLE drag carrying the file list) | Taken in; it showed that Tauri sends a drop as the webview's event with its `unstable` feature, not the window's |

## Known limitations

- **A file's own subtitle tracks are left out** of what is made, as are its extra sound tracks: a conversion keeps its picture and its default sound. Keeping them, or drawing them into the picture for Share, is still to decide.
- **A conversion's size is a range** until it is made: a file's size is not measured on it as a link's is (`crate::sizing` reads a stream's link), though a copy's is the file's own. A list's total adds up each file's plan.
- **A smart cut falls back to a full re-encode** where a file's own timestamps have a gap (a recording joined from pieces) or the part has fewer than two key frames, as for a download. The clip is right either way, only slower.
- **The preview copy of a long file** takes a while: 17 s for twenty minutes of 1080p.
