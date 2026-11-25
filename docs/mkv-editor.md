# MKV editor (planned)

Planned 2026-11-25 with the owner; not built yet. Choosing which tracks an MKV keeps, adding tracks from other files, and fixing their details, for one film or a whole season, without losing any quality: every track is copied as it is, never converted. It belongs with [Files on this PC](local-files.md).

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
| Picture subtitles (PGS, VobSub) | **Read with Windows' own OCR** (`Windows.Media.Ocr`, part of Windows, nothing to download): a few dozen of its pictures are drawn with FFmpeg, black on white, read, and their words told as above. Windows reads only the languages installed on the PC; a track in another comes out as nonsense, and is unknown |
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
   - 2b: text subtitles read for their language, SDH and forced lines.
   - 2c: picture subtitles read with Windows' OCR.
   - 2d: the list in the window.
3. **One file, edited**: remove, change details and order; make it in `Edited`, check it.
4. **My languages**: the list in Settings and the rules above, with unit tests for the hard cases.
5. **Seasons**: the files list, one choice for every file, marks where something is uncertain.
6. **Adding tracks**: to one file, then to a season by name.
7. **Chapters and fonts.**
