/**
 * Types shared with the Rust backend. Each mirrors a `Serialize`/`Deserialize`
 * struct in `src-tauri/src`; the Rust side is the source of truth.
 */

// ── error.rs ────────────────────────────────────────────────────────────

export type ErrorKind =
  | "drm"
  | "unsupported"
  | "unavailable"
  | "login_required"
  | "outdated"
  | "network"
  | "disk_full"
  | "cancelled"
  | "setup"
  | "busy"
  | "too_long"
  | "internal";

export interface AppError {
  kind: ErrorKind;
  /** Plain-language explanation, safe to show as-is. */
  message: string;
  /** Raw log for the "details" toggle. */
  details: string | null;
  /** Set when signing in to a site can fix this. */
  sign_in: SignInNeed | null;
}

/**
 * How likely signing in is to help (`error::Chance`): a login wall, only if
 * the account has access, the bot check (risky for the account), or an
 * error Siphon doesn't recognise.
 */
export type SignInChance = "needed" | "maybe" | "risky" | "unknown";

export interface SignInNeed {
  /** "youtube.com" */
  site: string;
  chance: SignInChance;
  /** Siphon is signed in to the site already. */
  signed_in: boolean;
  /** Refused even with the sign-in: for "needed" it ran out, for "maybe" the account has no access. */
  refused: boolean;
}

// ── sessions/ ───────────────────────────────────────────────────────────

/** How to sign in to a site (`sessions::Method`). */
export type SignInMethod = { type: "window" } | { type: "browser"; browser: string } | { type: "file" };

/** A browser on this computer a sign-in can be taken from (`sessions::browsers::Browser`). */
export interface Browser {
  /** yt-dlp's name: "firefox", "chrome", … */
  id: string;
  name: string;
}

/** How a sign-in ended (`sessions::sign_in::Outcome`). */
export type SignInOutcome =
  | { outcome: "signed_in"; site: string; tip: boolean }
  | { outcome: "refused"; short: string; reason: string }
  | { outcome: "cancelled" };

/** One signed-in site, for Settings. Holds no cookie. */
export interface SiteSession {
  site: string;
  method: SignInMethod;
  /** Unix seconds. */
  signed_in_at: number;
  last_used: number | null;
  expired: boolean;
  mode: SignInMode;
}

/** When a signed-in site's sign-in is used: only after the site refused without it, or every time. */
export type SignInMode = "when_needed" | "always";

/** The sign-in window's state, for its bar. */
export type SignInStatus =
  | { phase: "waiting" }
  | { phase: "checking" }
  | { phase: "not_yet" }
  | { phase: "failed"; message: string };

// ── binaries/mod.rs ─────────────────────────────────────────────────────

export type Tool = "yt_dlp" | "ffmpeg" | "deno";

export type ToolState =
  | "missing"
  | "checking"
  | "downloading"
  | "verifying"
  | "installing"
  | "ready"
  | "failed";

export interface ToolStatus {
  tool: Tool;
  state: ToolState;
  version: string | null;
  /** 0–1 while downloading. */
  progress: number | null;
  error: string | null;
  /** A newer version waiting for the user's go-ahead ("ask first" mode). */
  available: { version: string; summary: string } | null;
}

/** Sent after a tool was updated (`binaries://updated`). */
export interface UpdateNotice {
  tool: Tool;
  from: string | null;
  to: string;
  /** One line about what changed, from the release notes. */
  summary: string;
}

// ── ytdlp/metadata.rs ───────────────────────────────────────────────────

export type FormatKind = "video" | "audio" | "muxed";

export interface Format {
  id: string;
  ext: string;
  kind: FormatKind;
  /** null when the stream has no such track. */
  video_codec: VideoCodec | null;
  audio_codec: AudioCodec | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  /** kbit/s */
  tbr: number | null;
  vbr: number | null;
  abr: number | null;
  asr: number | null;
  audio_channels: number | null;
  /** Bytes. */
  filesize: number | null;
  filesize_estimated: boolean;
  dynamic_range: string | null;
  language: string | null;
  language_preference: number | null;
  /** True for segmented (HLS) streams. */
  segmented: boolean;
  note: string | null;
}

export interface VideoInfo {
  id: string;
  title: string;
  url: string;
  thumbnail: string | null;
  /** Seconds. */
  duration: number | null;
  channel: string | null;
  view_count: number | null;
  formats: Format[];
  /** Found with the site's sign-in, because the video needed it (or the user asked). */
  with_account: boolean;
}

export interface PlaylistEntry {
  id: string;
  title: string;
  url: string;
  duration: number | null;
  thumbnail: string | null;
  channel: string | null;
  /** A link to another playlist (a channel's Playlists tab), opened rather than downloaded. */
  is_playlist: boolean;
  /** The part of a channel it was listed under ("Videos", "Live", "Shorts"); null for a plain playlist. */
  section: string | null;
}

export interface PlaylistInfo {
  id: string;
  title: string;
  url: string;
  channel: string | null;
  entries: PlaylistEntry[];
}

export type Media = ({ type: "video" } & VideoInfo) | ({ type: "playlist" } & PlaylistInfo);

// ── plan ───────────────────────────────────────────────────────────────
//
// Groups, targets and tiers are defined once, in `plan/registry.rs`, and read
// through `get_presets`. Their ids are plain strings here on purpose: the
// interface never lists them itself.

export type GroupId = string;
export type TargetId = string;
export type TierId = string;

export type VideoCodec = "h264" | "hevc" | "vp9" | "av1" | "vp8" | "prores" | "other";
export type AudioCodec = "aac" | "opus" | "mp3" | "vorbis" | "flac" | "ac3" | "pcm" | "other";
export type Container =
  | "mp4"
  | "mkv"
  | "webm"
  | "mov"
  | "m4a"
  | "mp3"
  | "opus"
  | "ogg"
  | "flac"
  | "wav";
export type Content = "video_audio" | "video_only" | "audio_only";
export type MusicFormat = "mp3" | "m4a";
export type QualityKind = "resolution" | "bitrate" | "none";
export type Extra = "tier" | "smooth_editing" | "music_format";

export interface Tier {
  id: TierId;
  label: string;
  /** Megabytes (10⁶ bytes). */
  limit_mb: number;
  max_lines: number;
  /** False when the file is sent as a document rather than played in the chat. */
  inline: boolean;
}

export interface Target {
  id: TargetId;
  label: string;
  /** An icon name; see `src/lib/icons.ts`. */
  icon: string;
  quality: QualityKind;
  extras: Extra[];
  tiers: Tier[];
}

export interface Group {
  id: GroupId;
  label: string;
  icon: string;
  description: string;
  targets: Target[];
  default_target: TargetId;
}

/** Every preset Siphon offers (`get_presets`). */
export interface Registry {
  groups: Group[];
  music_bitrates: number[];
}

/** null means "best available" for streams and "keep the original" for codecs and container. */
export interface ManualOptions {
  content: Content;
  video_format: string | null;
  audio_format: string | null;
  container: Container | null;
  video_codec: VideoCodec | null;
  audio_codec: AudioCodec | null;
}

/** What to download: a preset from the registry, or custom settings. */
export type Choice =
  | {
      mode: "preset";
      target: TargetId;
      /** A resolution, or a bitrate for Music; null is "Best". */
      quality: number | null;
      tier: TierId | null;
      music_format: MusicFormat;
      smooth_editing: boolean;
    }
  | ({ mode: "custom" } & ManualOptions);

/** The part of the video to keep, in seconds. Always cut on the exact frames. */
export interface Trim {
  start: number;
  end: number;
}

export interface Hint {
  level: "info" | "warning";
  text: string;
}

export type Badge = "lossless" | "converts" | "size_optimized" | "may_not_play";

export interface QualityOption {
  /** A resolution (short side), or a bitrate for Music. */
  value: number;
  /** Picking this means converting. */
  converts: boolean;
  estimated_size: number | null;
  /** Low and high, when the size depends on the picture. */
  size_range: [number, number] | null;
}

export interface PlanSummary {
  badge: Badge;
  headline: string;
  detail: string;
  /** Bytes, when predictable: the likely value. */
  estimated_size: number | null;
  /** Low and high, when the size depends on the picture (a conversion): shown instead. */
  size_range: [number, number] | null;
  /** Roughly how long converting takes, in seconds. */
  convert_seconds: number | null;
  hints: Hint[];
  quality: QualityKind;
  options: QualityOption[];
  /** The quality this plan came to. */
  resolved: number | null;
  ext: string;
  /** Why Download isn't offered (Share: too long to look decent). */
  blocked: string | null;
}

// ── choices.rs ─────────────────────────────────────────────────────────

export type Active = { kind: "preset" } | { kind: "custom" } | { kind: "saved"; id: string };

export interface SavedPreset {
  id: string;
  name: string;
  options: ManualOptions;
}

/** What Siphon remembers between downloads (`choices.json`). */
export interface Choices {
  active: Active;
  group: GroupId;
  /** The target last used in each group. */
  targets: Record<GroupId, TargetId>;
  quality: number | null;
  music_kbps: number | null;
  /** The tier picked for each Share target. */
  tiers: Record<TargetId, TierId>;
  music_format: MusicFormat;
  smooth_editing: boolean;
  custom: ManualOptions;
  saved: SavedPreset[];
}

// ── settings.rs ────────────────────────────────────────────────────────

export type FilenameStyle = "title" | "title_id" | "channel_title" | "date_title" | "custom";

export interface Settings {
  /** Where downloads are saved; null means the system Downloads folder. */
  output_dir: string | null;
  filename: FilenameStyle;
  /** A yt-dlp output template without extension; used when `filename` is "custom". */
  custom_template: string;
  /** How many downloads run at once. */
  max_concurrent: number;
  /** yt-dlp's release channel. */
  update_channel: "stable" | "nightly";
  /** Install new versions of the tools without asking, or ask first. */
  update_mode: "auto" | "ask";
  /**
   * All downloads together get at most this many bytes per second; null is
   * unlimited. MB is 1,048,576 bytes here, as speeds are shown.
   */
  speed_limit: number | null;
}

/** What `get_settings` returns: the settings plus facts for displaying them. */
export interface SettingsView extends Settings {
  default_output_dir: string | null;
}

// ── preview.rs ─────────────────────────────────────────────────────────

export type PreviewSource =
  | { kind: "youtube"; id: string }
  | { kind: "stream"; url: string }
  | { kind: "file"; path: string };

export interface Preview {
  source: PreviewSource;
  /** Ask for level + 1 if this source fails to play. */
  level: number;
}

// ── downloads ───────────────────────────────────────────────────────────

export type Stage =
  /** Waiting for a free slot in the queue. */
  | "queued"
  /** yt-dlp is being updated before a second attempt. */
  | "updating"
  /** Waiting a moment before trying again after a failure that may pass. */
  | "retrying"
  | "preparing"
  | "downloading_video"
  | "downloading_audio"
  | "downloading"
  | "merging"
  | "converting"
  | "finishing";

export interface Progress {
  stage: Stage;
  /** Overall completion, 0–1. */
  fraction: number;
  /** Bytes per second. */
  speed: number | null;
  /** Seconds. */
  eta: number | null;
}

/** The total download speed (`speed://total`), measured as the bytes pass through Siphon. */
export interface Speed {
  bytes_per_second: number;
}

export interface DownloadRequest {
  id: string;
  url: string;
  choice: Choice;
  trim: Trim | null;
  place: Place | null;
  /** Use the site's sign-in from the start. */
  account: boolean;
}

/** Where a playlist video goes: a folder inside the download folder, and a number in front of its name. */
export interface Place {
  /** Usually the playlist's title; the backend makes it safe for a folder name. */
  folder: string | null;
  /** [position from 1, playlist length], for "07 - Title". */
  number: [number, number] | null;
}

export type DownloadEvent =
  | ({ type: "progress"; id: string } & Progress)
  | { type: "finished"; id: string; path: string; size: number | null }
  | { type: "failed"; id: string; error: AppError }
  | { type: "paused"; id: string }
  | { type: "cancelled"; id: string };
