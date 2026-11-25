# MKV editor

Planned 2026-11-25 with the owner, and being built in the steps at the end: one file can be read, edited and saved, with my languages applied (steps 1 to 4); seasons, adding tracks, and chapters and fonts are still to come. Choosing which tracks an MKV keeps, adding tracks from other files, and fixing their details, for one film or a whole season, without losing any quality: every track is copied as it is, never converted. It belongs with [Files on this PC](local-files.md).

## What it does

- **Remove** sound and subtitle tracks.
- **Add** subtitle, sound or video tracks from other files (SRT, ASS, SUP, MKA, AC3, DTS and the like).
- **Change** a track's language, name, which plays first (default), forced subtitles, and the tracks' order.
- **Chapters and fonts**: add, replace or remove chapters, and the font attachments ASS subtitles need.
- **One film or a whole season**: a folder of episodes opens as a list, and one choice, made by language and kind, applies to every file.

## What is made, and where

The original is never touched, as everywhere in Siphon. What is made keeps the original's name exactly, in a folder named `Edited` beside it: `Movies\Edited\Film (2010).mkv`, `Show\Season 1\Edited\Show S01E03.mkv`. Media servers (Plex, Jellyfin, Kodi) and release names read the title, year and episode from the name, so it isn't changed; the folder tells the two apart. A name already taken there gets `(2)`, as elsewhere. Written under the job's marker, checked, then renamed, as conversions are.

## My languages

An ordered list in Settings (1. Greek, 2. English); the subtitle language already chosen becomes its first entry. **It is applied as soon as a file or season opens**, with what it removes shown plainly, and every choice can still be changed by hand.

- **Sound:** every track in one of my languages is kept, the first language's plays first. When none is in my languages, the original sound alone is kept. The original is never kept just for being the original: an English film with a Greek dub keeps the English only if English is in the list. A file is never left without sound.
- **Subtitles:** those in my languages are kept, full and forced. With sound in my language, its forced subtitles play first and the full ones are kept, off. Otherwise the full subtitles in my first language play first.
- **Commentary** (sound or subtitles) in my languages is kept, never played first.
- **SDH** (subtitles for the deaf and hard of hearing) is left out, when it is certainly SDH: marked hearing-impaired in the file, or named SDH, CC or Hearing Impaired. Where it is the only full subtitle track in a language, it is kept, not played first.

## Certain before anything is left out

A track is only left out when Siphon is certain of its language. Anything else is kept and marked, never guessed. Subtitles are read for what they say, not only for what they are labelled (the owner's call, 2026-11-25); sound is taken by its label, as nothing short of a speech model can hear it.

| Track | How its language is known |
| --- | --- |
| Text subtitles (SRT, ASS, WebVTT) | **Read.** Their words are taken out with FFmpeg and their language told from them by a small detector built into Siphon (under 1 MB, nothing to download). An episode's hundreds of lines leave no doubt; Greek is plain from its alphabet alone |
| Picture subtitles (PGS, VobSub) | **Read with Windows' own OCR** (`Windows.Media.Ocr`, part of Windows, nothing to download): a dozen of its pictures are drawn with FFmpeg, dark on white, read by every reader Windows has, and their words told as above. A reading bears a tag out or names an untagged track, but never overrules a tag: a reader shown a script it doesn't know makes words of it all the same (Greek letters of Cyrillic), so a reading that disagrees with the tag casts doubt on it instead (kept, marked). Windows reads only the languages installed on the PC; a track in another is taken by its tag, as sound is, and says so |
| Sound | **Its tag.** No tag, or `und`: kept, marked *Language unknown*. A tag that looks wrong is kept and marked *Check this one*, never acted on: two sound tracks with the same language and channels, neither named commentary; a file named for a language (`EN-GR`) with no track tagged so; a name that says another language than the tag |
| No language element at all | English, by Matroska's own rule: MKVToolNix long left it out for English tracks, and every player reads it so (the Korra episodes' English tracks have none). For subtitles, read all the same |
| The track's name ("English", "Ελληνικά") | Never enough on its own. Shown, and a reason to mark a track when it disagrees |
| Which sound is the original | The file's original flag, else the one marked to play first; if neither settles it, every sound track is kept and the file marked |

What subtitles say settles more than their language:

- **A wrong tag is found and put right.** A track tagged English whose words are Greek is Greek, and the new file says so.
- **SDH** is known by its words too: sounds in brackets (`[door slams]`), `♪`, speakers named in capitals (`JOHN:`), besides the hearing-impaired flag or SDH in the name.
- **Forced subtitles** are known by having far fewer lines than the full track in the same language, besides the forced flag or Forced in the name. This only decides what plays first, never what is left out.

Tried 2026-11-25 on Korra S01E01: six of its English PGS pictures were read word for word by Windows' Greek and English readers alike ("Hey, you! Stop! You can't fish here!"), Greek lines by the Greek reader ("Δεν έχω λεφτά."), while the English reader made nonsense of them ("Atv éxw ÅEQTå."); about 12 ms a picture. Whisper (a speech model, 35–80 MB with what runs it) was weighed for sound and left out: tags on sound are nearly always right, untagged sound is kept anyway, and a small model's confident mistake would be worse than a mark.

In a season, a file with anything uncertain is marked (*2 tracks to check*), so nothing is left out unseen.

## Adding to a season

A folder of subtitle or sound files dropped on a season is matched to the episodes by name (`Show S01E03.el.srt` into `Show S01E03.mkv`, by the episode number or the same name). The language is read from the name (`el`, `ell`, `gre`, `Greek`) and checked against the text where it is text. What matches nothing is listed, to be placed by hand.

## Engine

Copying tracks is bound by the disk, not the processor: about a minute for 40 GB on an NVMe drive, the same with FFmpeg or MKVToolNix. Siphon's own FFmpeg costs no more space, and the tests below found it enough, so it is the engine (decided below). MKVToolNix was weighed and left out: the whole of 104.0 is 272 MB unpacked, and the parts Siphon would need, `mkvmerge.exe` (28.3 MB) and `mkvpropedit.exe` (22.5 MB), work alone; what it does better (editing a file in place, chapters beyond a plain list) is either ruled out by never touching the original or rare enough to be said rather than handled.

### Tried on a real episode

2026-11-25, *The Legend of Korra* S01E01 from the owner's library (290 MB, HEVC; Greek AAC marked to play first, English AAC "Surround", English AAC "Stereo-Commentary" with no commentary flag; Greek SRT, English PGS; four chapters), the English surround left out:

| | FFmpeg (`-map … -c copy`) | `mkvmerge` |
| --- | --- | --- |
| Time | 0.66 s | 0.95 s |
| Size | 248.7 MB | 243.2 MB (it laces sound frames) |
| Every kept track's packets | Identical to the original's, all five | Sound and subtitles identical; the video's packets rewritten, its decoded frames identical |
| Languages, names, chapters | Kept | Kept |
| Which tracks play first | **Greek subtitles turned on**, though none were: FFmpeg's Matroska writer marks the first track of a kind to play first when none is (`-default_mode infer`). `-default_mode passthrough`, with the flags set by Siphon, keeps them as chosen | Kept |
| The original | Unchanged (SHA-256) | Unchanged |

### Tried on the hard cases

2026-11-25, each made into an MKV by `mkvmerge` as releases are, then copied by FFmpeg as Siphon would (`-map 0 … -c copy -default_mode passthrough`) and compared:

| Case | Sample | FFmpeg's copy |
| --- | --- | --- |
| Dolby Vision profiles 5, 7 (with its enhancement layer, `hvcE`), 8.1 and 8.4 | FFmpeg's test suite (`fate-suite.ffmpeg.org`: `mov/dovi-p5.mp4`, `mkv/dovi-p7-hvce.mkv`, `mov/dovi-p81.mp4`, `hevc/dv84.mov`) | **Kept**: every video packet identical, and the `dvcC` and `hvcE` blocks byte for byte; FFmpeg only adds their optional names |
| HDR10+ (HEVC) | `hevc/hdr10_plus_h265_sample.hevc` | **Kept**: it travels inside the video packets, and the frame still carries it |
| ASS: two styles, `\pos`, `\fad`, `\k`, a Comment line, an event out of order, Greek text, `YCbCr Matrix` | Made here | **Kept**: extracted, byte for byte the file `mkvmerge` was given |
| Two fonts attached | Arial, Segoe UI | **Kept**: same bytes, names and `font/ttf` |
| Chapters: two editions, a nested chapter, a hidden one, names in two languages | Made here | **Flattened**: the second edition, the nested chapter, the hidden flag and the second language's names are lost; the names left are marked `und`. Plain chapters (one edition, one name each, as Korra's) are kept |

So FFmpeg holds for everything but chapters beyond a plain list, which it can neither read (ffprobe flattens them too) nor write.

**Decided (2026-11-25, the owner's call): FFmpeg alone, no MKVToolNix.** A file whose chapters are more than a plain list (more than one edition, nested or hidden chapters, names in several languages, ordered chapters) is found by Siphon's own reading of the file's Chapters element, a small EBML reader with no new dependency, and said before anything is made: "This file has 2 chapter versions; the copy keeps the first." Nothing is lost unsaid.

## Steps

Each a commit that leaves Siphon working.

1. ~~**Engine test**~~ Done 2026-11-25: FFmpeg (see above).
2. **One file, read**: its tracks, each with its language and how certain that is.
   - 2a, done 2026-11-25 (`local/mkv/`, command `read_tracks`): every track from ffprobe (tags read whatever their capitals or language suffix: `LANGUAGE`, `NUMBER_OF_FRAMES-eng`), flags from the file or the name (forced, SDH, commentary, description, original), attachments, the chapters' shape from the file's own Chapters element (`chapters.rs`, a small EBML reader), and the doubts on sound tags (`doubts.rs`). Read over all 52 Korra episodes: four layouts, no doubt raised; a test file made to fail every check was caught on each. The Airbender episodes, named `.mp4`, are Matroska inside, and are read as MKVs: Siphon goes by what a file is, not its name.
   - 2b, done 2026-11-25 (`local/mkv/words.rs`, with `whatlang`, MIT, one dependency): each text subtitle track's packets are printed by ffprobe (`-show_data`), the whole file when it is under a gigabyte, else four windows spread across it, a gigabyte in all (`-read_intervals`, one pass), and read: its language told from its words (needing 8 lines, or fewer in an alphabet only one language uses), a wrong tag put right, a tag between languages too close to tell apart (Danish and Norwegian, Serbian and Croatian…) left standing; SDH by lines that describe sounds (8% of 20 or more); forced by a quarter of the lines, or fewer, of the fullest track in its language. Tried on a file made of Korra's own subtitles with a trap in each track: Greek tagged English was put right, untagged English read as English, SDH without its flag and a 32-line track beside 384 found; and on all 52 episodes, every SRT and ASS track read as its tag says, in 0.1 s a file.
   - 2c, done 2026-11-25 (`local/mkv/pictures.rs`, the `windows` crate already in the build): a picture track's packets are listed in the same stretches as text's; twelve that show a line, spread over them, are drawn by FFmpeg on a white page without the video (`[0:N]negate` over `color`, 0.1 s each, four at a time) and read by each of Windows' readers. Tried on the 25 Blu-ray Korra episodes: every English PGS track borne out as English, about a second a track; the same track tagged Greek was marked "Windows read its pictures as English, but it is tagged Greek.", and untagged, read as English.
   - 2d, done 2026-11-25 (`src/features/tracks/TracksPanel.tsx`): **Tracks** ended a file's row (Watch, Audio, Share, Customize, Tracks; a card tucked behind the file's card since, see *Where it sits in the window*) when the file is Matroska inside (`LocalFile::matroska`, from ffprobe, not the name); chosen, it puts the choices for converting, Trim and the button away and lists the tracks under Video, Sound and Subtitles: each its language (or *Language unknown*), codec, channels, name and flags, and a mark for how its language is known, with the reasons on hover: a check where its words or pictures were read ("Windows read its pictures: English."), a tag where it is taken by its tag, a warning where something casts doubt ("It is kept: Siphon leaves out only what it is sure of."). The file's own doubts and the chapters' note follow as notes. Tried in the dev build on Korra S01E01 (listed in 0.7 s, OCR included), on a file of traps and on one made to cast every doubt, in both themes.
3. **One file, edited**: remove, change details and order; make it in `Edited`, check it.
   - Done 2026-11-25. In the window (`src/features/tracks/`): a box on each row keeps or leaves out its track (left out: struck through, dimmed); the row opens to its language (a list of 35 and its own), its name, the flags that matter for its kind (sound: plays first, original, commentary; subtitles: plays first, forced, SDH) and arrows to move it among its kind; marking one to play first unmarks the others of its kind. A line says what saving makes ("Keeps 5 of 6 tracks · No quality loss", ≈ size from the tracks' own byte counts), and the button what it does: *Nothing to change*, *Save without 1 track*, *Save the changes*, *Keep at least one track*. A language read from a track's words is the one written.
   - The job (`DownloadRequest::tracks`, `DownloadManager::run_tracks`): a file job like a conversion (Activity, cancel, resume, the notification: *Copy finished*), with a disk turn (`Load::Disk`). It checks the edit against the file, makes `Edited` beside it (taken away again if the copy fails and it is empty; the download folder if it can't be written, with the note), checks there is room, and copies with FFmpeg (`local::mkv::edit::args`: the kept tracks mapped in order, attachments and chapters with them, `-c copy`, `-default_mode passthrough`, each track's `language` in Matroska's codes, `title`, and every flag as `-disposition`) under the job's marker; the copy is renamed only once it holds the kinds of track asked for, in order, and the whole length (`check_copy`). Progress is FFmpeg's, as *Copying its tracks*.
   - Tried 2026-11-25 on a copy of Korra S01E01 in a scratch folder (never in the library): the English surround left out and the commentary renamed, from the window. 2.2 s with the checks; the copy held video, Greek (plays first), English *Commentary* (flagged), Greek SRT, English PGS and the four chapters, each track's packets identical to the original's; the original's SHA-256 unchanged. Saved again with the English PGS moved above the Greek SRT and marked forced: *Copying its tracks*, then *Saved*, as `Korra S01E01 (2).mkv` beside the first, nothing half-written left. A flag known only from a track's name ("Stereo-Commentary") is written as a flag in the copy, so players know it too.
4. **My languages**: the list in Settings and the rules above, with unit tests for the hard cases.
   - Done 2026-11-25. Settings has a **Languages** section: the user's languages in order (`Settings::languages`, kept as the language alone, "el", each once), each movable earlier or removable, and a list to add one. Until chosen, the subtitle language chosen for downloads and then Windows' languages (`ApplicationLanguages::Languages`, in Windows' order) stand in (`settings::default_languages`), and the section says so.
   - The rules are `local::mkv::pick::suggest`, a pure function over what reading found and the languages, with a test for each case: Korra for Greek then English (all kept, Greek plays first), for English first (English plays first, and it says so), for Greek alone (the English left out: sound, commentary, subtitles); a film with no sound in my languages (its original kept, my subtitles play); no original known (all sound kept); forced subtitles first with sound in my language; SDH left out unless a language's only full subtitles; uncertain tracks kept; commentary never first. Each track left out comes with why ("English commentary: not one of your languages."), and what else changed with a line.
   - `read_tracks` brings the suggestion with the file, and the window applies it as the file opens: a note over the tracks says *Kept for your languages: Greek, English.*, what was left out and why, what plays first now, with **Keep everything** to go back to the file as it is (and **Keep my languages** to apply them again), and a link to Settings. A file all in my languages says so in a line. Changing the languages reads the file again and applies them afresh.
   - Tried 2026-11-25 in the dev build, on a copy of Korra S01E01 in a scratch folder: with the stand-in languages (English then Greek, from Windows), the English surround was set to play first and the note said why; English removed in Settings, the file was applied afresh at once, its English sound, commentary and subtitles left out with their reasons and the button *Save without 3 tracks*; *Keep everything* gave back all six, *Keep my languages* the three again; saved in 2 s as video, Greek (plays first), Greek SRT and the four chapters. Five section tabs no longer fit Settings' width at equal shares: each now takes its own width (`flex-auto px-1`).
5. **Seasons**: the files list, one choice for every file, marks where something is uncertain.
   - Done 2026-11-26 (`src/features/tracks/season.ts`, `SeasonTracks.tsx`, in `FilesPane`). Files dropped or picked together open as the list they always did; when some are MKVs with tracks to choose, the same tracks card is tucked behind the list's header card (*Tracks of all 12 files*, their languages from their tags). Pulled out, it reads every MKV of the list (`read_tracks`, as a single file's, three at a time by the backend) and opens each with what the user's languages keep, as a single file does. The presets and their note fold away; the list stays, and the button becomes *Save 12 copies*.
   - The season's tracks are shown by kind, language and role, across its files: *Greek* sound, *English · Commentary*, *Greek* subtitles, *English · SDH*…, each *In every file (12)* or *In 11 of 12 files*. Ticking or unticking one keeps or leaves it out in every file that has one (a box shows *some* when files differ); *Plays first* makes it play first in each, and the others of its kind not. *Keep everything* and *Keep my languages* apply to every file. A track whose language isn't certain belongs to no row, so no season choice touches it: it is kept, and its file marked *1 to check* in the list, with why on hover.
   - In the list, each MKV says what its copy keeps (*Keeps 4 of 6*, or *Nothing to change*, which can't be selected) and opens under its row, by its arrow, to its own tracks, to change by hand as a single file's. A file that isn't an MKV says *No tracks to choose*. Saving starts a copy of each selected file (the same job as a single file's), each in an `Edited` folder beside it.
   - Tried 2026-11-26 in the dev build, on copies of Korra Book 1 in a scratch folder (12 episodes, 2.7 GB), E12's English surround untagged to plant a doubt: the card read *Tracks of all 12 files · Sound and subtitles in Greek and English*; pulled out, all 12 read in about 12 s, *English* sound *In 11 of 12 files* and the warning for E12. English sound and English subtitles left out for the season: 11 files *Keeps 4 of 6*, E12 *1 to check · Keeps 5 of 6*, its untagged surround kept; opened in the list and left out by hand, *Save 12 copies* made all 12 in `Book 1\Edited` in about 20 s, each with the video, Greek sound (plays first), the English commentary and the Greek subtitles; every original's SHA-256 unchanged.
6. **Adding tracks**: to one file, then to a season by name.
7. **Chapters and fonts.**

## Where it sits in the window

**A card tucked behind the file's card** (2026-11-26, the owner's call, `src/features/tracks/TracksCard.tsx`). Its lower edge shows under the file's card: the layers icon, *6 tracks* (or *5 of 6 tracks kept* once chosen from), the languages ("Sound and subtitles in Greek and English", from the file's tags until its tracks are read, `LocalFile::overview`), and **Edit tracks**. It is shown only for a file with more to choose than one picture and one sound. It peeks out once when the file opens, and a little more under the pointer, to show it can be pulled. Pressed, it slides out from under and takes the full width, the file's card above it shrinks to a slim header (its picture from 208 to 112 px wide), and the conversion (presets, facets, plan, Trim) folds away; the button becomes *Save without 1 track*. *Back to converting* in its header reverses it all. Motion uses a drawer's easing (`--ease-drawer`) and is still under Reduce motion.

How it got here: Tracks was first a fifth choice in the presets' row, where it read as one more thing to make beside Trim; then a small chip among the card's description chips (shipped in 0.21.0), which the owner found too small and too secondary for a feature this big. Three ways in were then drawn as prototypes: the file's card unfolding into the tracks, a track map drawn like an editor's timeline, and this deck of two cards. The owner chose the deck; the track map was too busy. In the prototype a third card peeked under the second for depth; it read as a dark band, like a bug, and was left out.

Tried 2026-11-26 in the dev build, on a copy of Korra S01E01, in both themes: the edge said *6 tracks* and its languages, opened and closed as above, said *5 of 6 tracks kept* after the English surround was left out, and saved the copy (the original's SHA-256 unchanged); a one-picture, one-sound MKV showed no card, and one with a subtitle track *3 tracks · Sound and subtitles in Greek*.

## Open questions

- **Order of the next steps** (2026-11-26, the owner's call): seasons (step 5) before adding tracks (step 6), as planned. Adding tracks, a subtitle file into an MKV say, is not built yet.
- **Tracks for more than MKV.** Proposed: MP4 and MOV stay MP4 (picture subtitles can't go in MP4: say so and offer MKV, as Customize does); TS and M2TS become MKV. Decided 2026-11-26: after the 0.21.0 release, not before.
