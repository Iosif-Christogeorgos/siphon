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

A track is only left out when Siphon is certain of its language. Anything else is kept and marked, never guessed.

| What tells a track's language | How it is taken |
| --- | --- |
| The language tag in the file | Trusted |
| No tag, or `und` | Unknown: kept, marked *Language unknown* |
| A text subtitle (SRT, ASS) without a tag | Its text is read and its language told from it; acted on only when the answer is clear (Greek from English is plain from the alphabet) |
| Sound or picture subtitles (PGS, VobSub) without a tag | Can't be told without recognising speech or text: always kept, marked |
| The track's name ("English", "Ελληνικά") | Never enough on its own to leave a track out. Shown; where it disagrees with the tag (tagged English, named Greek), the track is kept and marked *Check this one* |
| Which sound is the original | The file's original flag, else the one marked to play first; if neither settles it, every sound track is kept and the file marked |
| Which subtitles are forced | The forced flag, or Forced in the name; this only decides what plays first, never what is left out |

In a season, a file with anything uncertain is marked (*2 tracks to check*), so nothing is left out unseen.

## Adding to a season

A folder of subtitle or sound files dropped on a season is matched to the episodes by name (`Show S01E03.el.srt` into `Show S01E03.mkv`, by the episode number or the same name). The language is read from the name (`el`, `ell`, `gre`, `Greek`) and checked against the text where it is text. What matches nothing is listed, to be placed by hand.

## Engine

Copying tracks is bound by the disk, not the processor: about a minute for 40 GB on an NVMe drive, the same with FFmpeg or MKVToolNix. Siphon's own FFmpeg costs no more space and, tried 2026-11-25 on a test file, removed a track and added an SRT, chapters and a font correctly. It is used unless real films show it can't: Dolby Vision, Blu-ray PGS, ASS with its fonts, chapters with editions. If one fails, `mkvmerge` (28 MB, from MKVToolNix's own downloads, checked against their `sha256sums.txt`) is fetched the first time the editor needs it, never at first launch. Measured 2026-11-25: the whole MKVToolNix 104.0 is 272 MB unpacked, `mkvmerge.exe` 28.3 MB and `mkvpropedit.exe` 22.5 MB, and both work alone, without its other files.

## Steps

Each a commit that leaves Siphon working.

1. **Engine test** on real films: Dolby Vision, PGS, ASS with fonts, chapters with editions. Decide FFmpeg or `mkvmerge`.
2. **One file, read**: its tracks, each with its language and how certain that is.
3. **One file, edited**: remove, change details and order; make it in `Edited`, check it.
4. **My languages**: the list in Settings and the rules above, with unit tests for the hard cases.
5. **Seasons**: the files list, one choice for every file, marks where something is uncertain.
6. **Adding tracks**: to one file, then to a season by name.
7. **Chapters and fonts.**
