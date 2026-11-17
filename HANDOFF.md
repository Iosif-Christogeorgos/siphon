# Handoff: preset rework (branch `refactor/presets`)

The owner asked for the flat preset chips to be replaced by a purpose-based
system (groups → targets → quality), as a full replacement with nothing of the
old system left. The full brief was in the conversation; its essentials are
repeated under "Next steps" so this file stands on its own.

## State

- **Branch:** `refactor/presets`, pushed. `main` is untouched (v0.6.0 + one unreleased commit).
- **Done (commit f925cb5): the whole backend.** `cargo test` (71 tests) and
  `cargo clippy --all-targets -- -D warnings` pass.
- **Not started: the frontend.** It still calls the old commands, so **the app
  builds but planning and downloading fail at runtime on this branch**
  (`plan_download` now takes `choice`, `start_download` takes `request.choice`).
  Do not merge until the frontend is done.

## What the backend now has

| File | What it does |
| --- | --- |
| `plan/registry.rs` | **The single source of truth**: `Group` (watch, edit, audio, max, share), `TargetId` (windows, apple, android, tv, premiere, resolve, capcut, vegas, music, original, max, discord, whatsapp, social), `TierId` + limits, `Extra` (tier, smooth_editing, music_format), `QualityKind`, `MUSIC_BITRATES`. `registry()` is what the UI gets. |
| `plan/mod.rs` | `Choice` (`{mode:"preset", target, quality, tier, music_format, smooth_editing}` or `{mode:"custom", …ManualOptions}`), `Capabilities`, `Recipe` (now with `badge`, `size_limit`, `fits`, `blocked`, `section_request_size`, `fallback`), stream ranking, `plan()`, `quality_options()`, `Recipe::shrunk/on_software`. |
| `plan/watch.rs` `edit.rs` `audio.rs` `share.rs` `custom.rs` | One file per group. Share has the bits-per-pixel ladder (`pick_rung`). |
| `plan/explain.rs` | `summarize(video, choice, trim, caps)` → `PlanSummary` (below). |
| `choices.rs` | `Choices` in `choices.json` + `from_legacy()` (the **only** place old preset ids may appear; it is migration code). |
| `state.rs` | `CapabilityCache` (encoders, ffmpeg `-request_size`, Windows VP9/AV1/HEVC extensions via `Get-AppxPackage`, ~100 ms), `MediaCache` now evicts oldest at 512. |
| `downloads/mod.rs` | Sources for conversions go to `<app cache>/work/<id>/`, kept until the output is written and checked; `convert_checked` writes `X.converting.ext`, checks Share size, re-encodes smaller up to 2×, then renames. `sweep_work` at launch removes work folders older than a day. |
| `ffmpeg/mod.rs` | `convert_args(recipe, input, output, Pass)`; ProRes, `-g`, `-ar`, `-ac 2`, NVENC `-multipass`, two-pass x264. Input options have their own section (for `-rw_timeout` later). |
| `error.rs` | New `ErrorKind::TooLong` (frontend `ErrorCard` needs an entry: e.g. "Too long for this limit", icon `Scissors`). |

### IPC contract for the frontend

- `get_presets()` → `{ groups: [{ id, label, icon, description, default_target, targets: [{ id, label, icon, quality: "resolution"|"bitrate"|"none", extras: ("tier"|"smooth_editing"|"music_format")[], tiers: [{ id, label, limit_mb, max_lines, inline }] }] }], music_bitrates: number[] }`. Icon names: monitor, apple, smartphone, tv, clapperboard, music, audio-lines, gem, message-circle, phone, sparkles, play, scissors, headphones, send. lucide has no `apple`; map it to e.g. `Laptop`/`Smartphone`.
- `get_choices()` / `set_choices(choices)` → `Choices`: `{ active: {kind:"preset"}|{kind:"custom"}|{kind:"saved", id}, group, targets: {group: target}, quality: number|null, tiers: {target: tier}, music_format: "mp3"|"m4a", smooth_editing, exact_cut, custom: ManualOptions, saved: [{id, name, options: ManualOptions}] }`.
- `import_legacy_choices(old)`: call once at startup with the parsed `state` of localStorage `siphon.options` if it exists, then remove that key. It only applies when `choices.json` doesn't exist yet.
- `plan_download(url, choice, trim)` → `PlanSummary { badge: "lossless"|"converts"|"size_optimized"|"may_not_play", headline, detail, estimated_size, convert_seconds, hints, quality: kind, options: [{ value, converts, estimated_size }], resolved, ext, blocked }`.
- `estimate_download(url, choice)` → bytes or null; looks the video up if needed (3 at a time). For the playlist total.
- `start_download({ id, url, choice, trim })`.

## Next steps, in order

1. **Frontend types and stores.** In `types.ts`, replace `PresetId`/`PresetOptions`/`DownloadOptions`/`PlanSummary`/`QualityOption` with `Choice`, `Registry`, `Choices`, the new `PlanSummary`, and `ErrorKind` `too_long`. Type target, group and tier ids as `string` (the registry defines them; don't copy the lists). Replace `stores/options.ts` with a choices store loaded from `get_choices` and saved with `set_choices` (debounced). Add a registry query (`get_presets`, staleTime ∞). Do the legacy import in `App` startup. `exactCut` moves into choices.
2. **UI (restructure, don't restyle):** above the Download button:
   1. Groups row: a compact `Segmented` with icons (Watch · Edit · Audio · Max quality · Share).
   2. Targets row: chips for the group, spring-animated in (AnimatePresence keyed by group, no layout jump). For Max quality, show the group `description` instead. Extras inline and compact: tier (small segmented or select), "Smoothest editing (large files)" toggle, MP3/M4A.
   3. Quality row (`summary.quality`): "Best" (default, `quality: null`) then `options`, each with its size as small secondary text or a tooltip; bitrates for Music; hidden when `none`.
   4. "My presets" row only when `saved` is non-empty.
   5. Summary line (`PlanCard`, keep the look): badge icon per `badge` (lossless Zap, converts RefreshCw, size_optimized e.g. Minimize2, may_not_play TriangleAlert), headline, size with a subtle number transition, `convert_seconds` as "~1 min", detail and hints. When `blocked`, show it and disable Download, with a **Trim** button that opens the trimmer. A quiet **Customize** link near the summary opens `ManualPanel` (`active: custom`), which gets **Save as preset** (name field) and a way back to presets.
   - Delete `PresetPicker.tsx`, `QualityPicker.tsx` and the "Custom" chip. Use arrow keys in the rows (radiogroup), clear focus states, honour reduce motion.
   - Playlist: same group, target and extras; the quality row uses a fixed ladder (2160/1440/1080/720/480/360) without sizes; show the total from `estimate_download` per selected entry ("≈ 4.2 GB · 12 of 20 checked").
3. **Tooling:** add ESLint (flat config, typescript-eslint, react-hooks) and knip as dev dependencies with `lint` / `knip` scripts, and fix what they find.
4. **Verify nothing old is left:** `rg -i "PresetId|preset_id|max_lines|size_limit_mb|SHARE_LIMITS|sizeLimitMb|Dnxhr|dnxhr|ChunkedRequests|EncoderCache|PresetPicker|QualityPicker|siphon.options|\biphone\b|\bmacos\b|\"editing\"|\"share\"|\"audio\""` should only hit `choices.rs` (migration), the `siphon.options` import, and the app's own config. Run `cargo test`, `cargo clippy --all-targets -- -D warnings`, `npm run typecheck`, ESLint, knip and `npm run tauri build`.
5. **Manual test matrix** (real links from docs/decisions.md#testing-notes, plus a 4K X/Twitter video): each Watch target at Best and at a lower quality (Apple copies on X, converts on YouTube 4K); Premiere and Resolve; Music and Best audio; Max quality; Share with a clip that fits, one that needs compressing, one too long (blocked), and a trimmed part; Customize → save → it appears and works; migration (put `{"state":{"preset":"share","sizeLimitMb":50,"mode":"preset","exactCut":true,"manual":{…}},"version":0}` in localStorage `siphon.options`, delete `choices.json`, start). Check a Share result is under its limit, and that a failed conversion's **Try again** skips the download.
6. **Docs:** replace `docs/device-support.md` with `docs/presets.md` (what each target does and why, with the sources below), rewrite the README's presets section, move this file's lasting parts into `docs/decisions.md`, then delete this file.
7. **Ship:** merge into `main`, bump the **minor** version (0.7.0) in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit `chore: bump to 0.7.0`, tag `v0.7.0` and push (the owner asked for this release).

## Decisions made, and why

- **Limits are the cautious end of conflicting sources** (a file made for a smaller limit still sends):
  - Discord: Free 10 MB, Nitro Basic 50 MB, Nitro 500 MB. Several late-2026 blogs say Free went to 20 MB and Nitro to 1 GB, citing Discord's FAQ; this couldn't be confirmed.
  - WhatsApp, from its help centre: as media 100 MB at 720p (HD), 64 MB at 480p on slower connections; documents 2 GB, not played inline.
  - Social: TikTok 72 MB (the lowest app limit, Android), Reels 300 MB, Shorts 1 GB, all capped at 1080p.
  - Sources: [faq.whatsapp.com](https://faq.whatsapp.com/453914586839706/?cms_platform=web), [usecarly Discord](https://www.usecarly.com/blog/discord-file-size-limit/), [sto.care](https://sto.care/file-too-big-for-discord/), [vidkit](https://vidkit.eu/whatsapp-video-size-limit/), [socialpilot](https://www.socialpilot.co/insights/social-media-video-sizes-cheat-sheet), [mallary](https://mallary.ai/blog/social-media-video-specs).
- **Edit always re-encodes**, because no metadata says whether web video has a constant frame rate. Standard is H.264 CFR with a key frame every second plus AAC 320k at 48 kHz in MP4. Resolve Free on Windows reads 8-bit H.264; HEVC there depends on Windows' extension, so it isn't used.
- **The intermediate is ProRes 422 + PCM in MOV, replacing DNxHR**, because VEGAS's DNxHR import is unconfirmed while its ProRes import is documented. Resolve and Premiere read ProRes on Windows. CapCut gets no smooth toggle, since its support couldn't be confirmed. Sources: [vegascreativesoftware forum](https://www.vegascreativesoftware.info/us/forum/please-add-avid-s-dnxhr-and-dnxhd-for-import--121400), [tvtechnology](https://www.tvtechnology.com/news/vegas-creative-software-adds-apple-prores-support), [davinciresolveclub](https://davinciresolveclub.com/davinci-resolve-supported-codecs-formats/).
- **Watch:**
  - Apple converts to HEVC only with a hardware HEVC encoder; otherwise it uses hardware H.264, which is much faster than software HEVC.
  - Windows copies VP9, AV1 or HEVC only when this PC has that extension, and says so.
  - Android's "Best" stops at 1080p; higher resolutions are in the quality row.
  - Every device caps at 2160p60 and prefers SDR.
- **Share:**
  - A copy is used only when an H.264 + AAC original fits and is at least as sharp as the encode would be.
  - Picture size goes before frame rate (720p30 over 540p60). The thresholds are bits per pixel: 0.07 to look good, 0.03 at the minimum, 0.2 as the ceiling.
  - On the CPU it encodes in two passes; on NVENC it uses `-multipass fullres`. The file is checked after encoding and re-encoded up to twice if it's over the limit.
  - A copied file that comes out over the limit is converted with the plan's `fallback` recipe.
- **Music** copies a track already in the chosen codec (YouTube's AAC for M4A) unless a lower bitrate is picked. It says so when a bitrate is above the source.
- **The remembered choice lives in the backend** (`choices.json`), so migration is tested in Rust and Settings can't overwrite it.
- **Work folders older than a day are swept at launch, not all of them**, because the dev and installed builds share the cache folder.

## Known issues and follow-ups

- **Not yet seen in the running app**: the backend has only been tested by unit tests. In particular the Share size check and re-run, two-pass x264, ProRes, `Get-AppxPackage` detection, and resuming from the work folder.
- **The playlist total costs one lookup per selected video**, three at a time.
- **Still open, deliberately, but kept easy:** a stalled trimmed download can hang (add `-rw_timeout` to the `ffmpeg_i` args in `ytdlp/args.rs`, or to the input section of `ffmpeg::convert_args`). Downloading the same video twice overwrites the first; names are decided in `args::output_template` and `downloads::converted_path`.
- **The size estimate for constant-quality conversions** is a rough codec-efficiency factor (`Recipe::estimated_size`).

## How to run and test

`npm install`, then `npm run tauri dev`. Checks: `npm run typecheck`, `cargo test --manifest-path src-tauri/Cargo.toml`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`. To drive the app over DevTools, see [docs/decisions.md](docs/decisions.md#testing-notes).
