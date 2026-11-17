# Presets

What each preset does, and where its assumptions come from. The presets themselves are defined once, in [`src-tauri/src/plan/registry.rs`](../src-tauri/src/plan/registry.rs); the planner files next to it (`watch.rs`, `edit.rs`, `audio.rs`, `share.rs`) hold the rules below. Last checked in November 2026.

Two rules apply throughout:

- **Decide from the video's own formats.** Nothing assumes what a site offers. YouTube has no H.264 above 1080p, X has H.264 at 2160p, so the same Apple preset copies on X and converts on YouTube.
- **When a claim couldn't be confirmed, take the cautious side**: convert rather than hand over a file that might not play, and aim for the smaller of two reported upload limits.

## Groups and targets

| Group | Targets | What it makes |
| --- | --- | --- |
| Max quality | (one click) | The best streams exactly as the site has them, in a container that needs no re-encoding |
| Watch | Windows · iPhone / iPad / Mac · Android · TV | The best video and audio the device plays as they are; converted only when nothing native is offered at the chosen quality |
| Share | Discord · WhatsApp · TikTok / Reels / Shorts | The best quality that fits the upload limit of the chosen tier |
| Audio | Music · Best audio (original) | MP3 or M4A at a chosen bitrate, or the original track untouched |
| Edit | Premiere Pro · DaVinci Resolve · CapCut · Vegas | Constant frame rate H.264 with 48 kHz audio, or ProRes ("Smoothest editing") |

The quality choice offers **Best** and the resolutions the video really has (no tiny ones unless that is all there is). Like the other choices under the groups, it shows only its current value until pointed at, then unfolds; the estimated size of the resolution pointed at shows at its end. A lower resolution downloads the matching stream; nothing is upscaled. Max quality and Share have no quality row; Music offers bitrates instead.

## Watch

| Target | Copied as-is | Container | Converted to |
| --- | --- | --- | --- |
| Windows | H.264 with AAC; VP9, AV1 or HEVC only if this PC has Microsoft's extension for it | MP4 (WebM for VP9) | H.264 (HEVC above 1080p if the HEVC extension is installed) |
| Apple | HEVC, H.264 with AAC | MP4 | HEVC tagged `hvc1` on the GPU; H.264 on the GPU if it has no HEVC encoder |
| Android | H.264, HEVC, VP9 with AAC | MP4 | H.264 up to 1080p, HEVC above |
| TV | H.264, HEVC with AAC | MP4 | H.264 up to 1080p, HEVC above |

All stop at 2160p60 and pick SDR over HDR when both are offered. Android's **Best** stops at 1080p (phone screens, and the decoders on cheaper phones); higher resolutions are in the quality row.

- **Apple, confirmed:** the iPhone 17 specifications list HEVC, H.264 and AV1 playback ([apple.com](https://www.apple.com/ae/iphone-17/specs/)). Photos and QuickTime don't open WebM or MKV or play VP9 ([Apple developer forums](https://developer.apple.com/forums/thread/664770)), and only accept HEVC tagged `hvc1` ([Apple discussions](https://discussions.apple.com/thread/8091782)). Whether Photos imports AV1 is unconfirmed, so AV1 is converted.
- **Android, confirmed:** the [supported media formats](https://developer.android.com/media/platform/supported-formats) table: H.264 always, HEVC since 5.0, VP9 since 4.4, AV1 since 10, all readable from MP4. AV1 is converted because only recent phones decode it in hardware.
- **Windows, confirmed:** H.264 plays out of the box; HEVC needs the paid HEVC Video Extensions unless the maker preinstalled them ([Windows Latest](https://windowslatest.com/2025/07/16/can-you-get-hevc-codec-for-free-on-windows-11)); VP9 and AV1 need free Store extensions. Siphon checks which are installed (`Get-AppxPackage`, about 0.1 s) and copies those codecs only when they are, saying so in the summary.
- **TV:** no single specification exists. H.264 in MP4 is the universal baseline and every 4K set decodes HEVC; VP9 and AV1 from USB are inconsistent, so they are converted.

## Edit

Every editor needs a constant frame rate and 48 kHz audio, and web video promises neither, so Edit always re-encodes:

- **Standard:** H.264 with a key frame every second (smooth scrubbing), constant frame rate, AAC 320 kbit/s at 48 kHz, in MP4. All four editors import it. DaVinci Resolve's free version on Windows reads 8-bit H.264 but HEVC only through Windows' own extension ([DaVinci Resolve Club](https://davinciresolveclub.com/davinci-resolve-supported-codecs-formats/)), so H.264 it is.
- **Smoothest editing (large files):** ProRes 422 with uncompressed 48 kHz audio in MOV; every frame stands on its own, at about ten times the size. Best defaults to 1080p for that reason. Premiere Pro and Resolve read ProRes on Windows; VEGAS Pro reads ProRes ([TV Tech](https://www.tvtechnology.com/news/vegas-creative-software-adds-apple-prores-support)) but its DNxHR import could not be confirmed ([VEGAS forum](https://www.vegascreativesoftware.info/us/forum/please-add-avid-s-dnxhr-and-dnxhd-for-import--121400)), which is why ProRes replaced DNxHR. CapCut doesn't offer the option: its support for intermediate codecs couldn't be confirmed.

## Audio

- **Music:** MP3 (the default, because everything plays it) or M4A. **Best** copies a track that is already in the chosen codec (YouTube's AAC track for M4A) and otherwise converts at a bitrate close to the source's. A chosen bitrate above the source's is allowed, with a note that it only makes the file larger.
- **Tags and cover art (all audio):** title, artist and date are written into the file, and the video's thumbnail becomes its cover (`--embed-metadata --embed-thumbnail`), so music players show more than a file name. WAV gets the tags but no cover; yt-dlp can't embed one there. Video files get neither: writing tags is one more pass over the whole file, which costs nothing for a song but a full copy of a long video.
- **Best audio (original):** the best track exactly as provided, in its natural container (Opus, M4A, …).

Siphon never presents a conversion as an improvement: no "lossless" FLAC from a lossy source.

## Share

The limits move often and the sources disagree; where they do, Siphon takes the smaller number, because a file made for a smaller limit still sends.

| Target | Tiers | Shown at most |
| --- | --- | --- |
| Discord | Free 20 MB · Nitro Basic 50 MB · Nitro 500 MB | 1080p |
| WhatsApp | Standard 64 MB · HD 100 MB · Document 2 GB (not played in the chat) | 480p · 720p · 2160p |
| TikTok / Reels / Shorts | TikTok 72 MB · Reels 300 MB · Shorts 1 GB | 1080p |

- **Discord:** free accounts can upload 20 MB (confirmed by the owner, November 2026). Nitro Basic is 50 MB across sources; Nitro is reported as 500 MB or 1 GB, so 500 MB ([usecarly](https://www.usecarly.com/blog/discord-file-size-limit/), [sto.care](https://sto.care/file-too-big-for-discord/)).
- **WhatsApp:** its help centre gives 100 MB at 720p for HD, 64 MB at 480p on slower connections, and 2 GB as a document ([WhatsApp](https://faq.whatsapp.com/453914586839706/?cms_platform=web)). Older guides still say 16 MB.
- **Short-form:** all three show 1080×1920 at most. TikTok's limit is the lowest app limit reported (72 MB, Android); Reels' is the API's 300 MB; Shorts has no practical limit ([SocialPilot](https://www.socialpilot.co/insights/social-media-video-sizes-cheat-sheet), [Mallary](https://mallary.ai/blog/social-media-video-specs)).

How a Share file is made (`share.rs`):

1. If an H.264 + AAC original fits under the limit (with 5% to spare) and is at least as sharp as an encode would be, it is **copied** (a trimmed part is smart-cut: see decisions.md).
2. Otherwise the budget is `limit × 0.95 ÷ length` (the trimmed length when a part is selected). Audio gets 128, 96, 64 or 48 kbit/s depending on how tight it is; video gets the rest, less 2% for the container.
3. The size and frame rate are chosen by **bits per pixel**: the largest picture (then the higher frame rate) that gets at least 0.07, so 720p30 wins over a blocky 1080p60. Never above the source or what the service shows. A generous limit isn't filled for nothing: 0.2 bits per pixel is the ceiling.
4. H.264 + AAC in MP4 with the index at the front, so it plays in the chat. On the GPU, NVENC runs its two-pass mode in one go; on the CPU, x264 runs two passes.
5. The finished file is checked against the limit. If it is over, it is encoded again at a proportionally lower bitrate (up to twice). An original that was copied but came out over is converted after all.
6. If even the smallest picture would get under 0.03 bits per pixel, the plan is **blocked**: the summary suggests trimming (with a Trim button) or a larger tier, instead of making a pixelated file.
