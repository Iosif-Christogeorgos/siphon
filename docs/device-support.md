# Device support behind the presets

Each device preset in [`src-tauri/src/plan/presets.rs`](../src-tauri/src/plan/presets.rs) encodes what that device plays without conversion. This page records where each claim comes from and how sure it is. It was last checked in November 2026.

Rule used throughout: when a claim could not be confirmed from a primary source, the preset takes the cautious side and converts, so you never get a file that might not play.

## Summary

| Preset | Copied as-is (no re-encoding) | Container | Converted to, when nothing native exists |
| --- | --- | --- | --- |
| iPhone | HEVC, H.264 with AAC | MP4 | H.264 up to 1080p, HEVC above |
| Mac | HEVC, H.264 with AAC | MP4 | H.264 up to 1080p, HEVC above |
| Android | H.264, HEVC, VP9, AV1 with AAC or Opus | MP4 | H.264 up to 1080p, HEVC above |
| Windows 11 | H.264 with AAC (MP4), VP9 with Opus (WebM), AV1 with AAC (MP4) | MP4 or WebM | H.264 |
| Smart TV | H.264, HEVC with AAC | MP4 | H.264 up to 1080p, HEVC above |
| Video editing | Nothing: always converted | MOV | DNxHR HQ, PCM audio, constant frame rate |
| Discord / WhatsApp | H.264 with AAC, if it fits the limit | MP4 | H.264 at a bitrate that fits |
| Audio only | The original audio stream | Its own (Opus, M4A, MP3) | Never |
| Maximum quality | The best streams of any codec | WebM, MP4 or MKV | Never |

All device presets stop at 2160p (4K) and 60 fps, and pick SDR over HDR when a site offers both.

## iPhone and Mac

- **Confirmed:** Apple's iPhone 17 technical specifications list HEVC, H.264 and AV1 for video playback ([apple.com](https://www.apple.com/ae/iphone-17/specs/)).
- **Confirmed by consistent community reports, not by an Apple specification:** the Photos app and QuickTime do not open WebM or MKV, and do not play VP9 ([Apple developer forums](https://developer.apple.com/forums/thread/664770), [Apple discussions](https://discussions.apple.com/thread/255118264)).
- **Confirmed by consistent community reports:** Apple players only accept HEVC whose MP4 tag is `hvc1`, not `hev1` ([Apple discussions](https://discussions.apple.com/thread/8091782)). Siphon sets `-tag:v hvc1` whenever it writes HEVC into MP4 or MOV.
- **Unconfirmed:** whether Photos imports AV1 video, and on which models. QuickTime plays AV1 only on Macs with a hardware decoder ([MacRumors forum](https://forums.macrumors.com/threads/av1-format-plays-in-quicktime-on-m4-mac-mini-but-not-m1-mac-mini-or-older-macbook-air.2482907/)). **Decision:** AV1 is treated as not playable and is converted.

## Android

All from the official [Android supported media formats](https://developer.android.com/media/platform/supported-formats) table:

| Codec | Decoder since | Readable containers |
| --- | --- | --- |
| H.264 | All versions | MP4, MKV, 3GP |
| HEVC | Android 5.0 | MP4, MKV |
| VP9 | Android 4.4 | WebM, MKV, MP4 |
| AV1 | Android 10 | MP4, MKV |
| AAC | All versions | MP4, M4A |
| Opus | Android 5.0 | Ogg, MP4, MKV |

**Decision:** everything goes into MP4. AV1 is the last choice because phones older than a few years decode it in software or not at all; the preset shows a note when it picks AV1.

## Windows 11

- **Confirmed:** H.264 with AAC plays out of the box.
- **Confirmed by press coverage of Microsoft's statements:** HEVC needs the paid "HEVC Video Extensions" unless the PC maker preinstalled a free copy ([Windows Latest](https://windowslatest.com/2025/07/16/can-you-get-hevc-codec-for-free-on-windows-11)). **Decision:** HEVC is never picked and is converted to H.264.
- **Reported, not confirmed for every PC:** VP9 and AV1 play through free Microsoft Store extensions that most installations already have. **Decision:** they are copied, and the preset shows a note naming the extension.

## Smart TV

TVs differ by brand and model year, and there is no single specification. H.264 with AAC in MP4 is the universal baseline, and 4K sets decode HEVC because broadcast and streaming 4K depend on it. **Decision:** VP9 and AV1 are converted, since USB and DLNA playback of them is inconsistent.

## Video editing

- **Confirmed:** DaVinci Resolve's free version on Windows decodes only the H.264 and HEVC profiles that Windows itself supports ([Blackmagic codec list](https://documents.blackmagicdesign.com/SupportNotes/DaVinci_Resolve_20_Supported_Codec_List.pdf)).
- **Confirmed:** Premiere Pro documents variable frame rate as a special case with its own handling ([Adobe](https://helpx.adobe.com/premiere-pro/compatibility.html)); web video is often variable.

**Decision:** convert to DNxHR HQ with uncompressed audio in MOV at a constant frame rate. Both editors read it on every platform without add-ons, and every frame is a key frame, so scrubbing and cutting are exact. The files are large (about 1.6 GB per minute at 1080p30), which the preset states up front. It defaults to 1080p for that reason.

## Discord and WhatsApp

Upload limits change often, and the sources disagreed when checked:

| Service | Limit | Confidence |
| --- | --- | --- |
| Discord, no Nitro | 10 MB; several 2026 sources say it was raised to 20 MB | Sources conflict |
| Discord, Nitro Basic | 50 MB | Consistent across sources |
| Discord, Nitro | 500 MB; some 2026 sources say 1 GB | Sources conflict |
| WhatsApp, video sent as media | 16 MB historically; recent sources say 64 MB to 100 MB | Sources conflict |

Neither help center could be read directly (Discord's blocks automated requests; WhatsApp's article URLs had moved). **Decision:** the preset offers 10, 16, 50, 100 and 500 MB and defaults to 10 MB, the smallest. Siphon aims 4% under the chosen limit.

If an original H.264 + AAC stream already fits, Siphon copies it. Otherwise it converts to H.264 at a bitrate computed from the video's length.
