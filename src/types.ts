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
  | "internal";

export interface AppError {
  kind: ErrorKind;
  /** Plain-language explanation, safe to show as-is. */
  message: string;
  /** Raw log for the "details" toggle. */
  details: string | null;
}

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
}

export interface PlaylistEntry {
  id: string;
  title: string;
  url: string;
  duration: number | null;
  thumbnail: string | null;
  channel: string | null;
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

export type PresetId =
  | "iphone"
  | "android"
  | "windows"
  | "macos"
  | "tv"
  | "editing"
  | "share"
  | "audio"
  | "max";

export type VideoCodec = "h264" | "hevc" | "vp9" | "av1" | "vp8" | "dnxhr" | "other";
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

export interface PresetOptions {
  mode: "preset";
  preset: PresetId;
  /** A specific resolution (short side); null lets the preset decide. */
  max_lines: number | null;
  /** Share preset only. */
  size_limit_mb: number | null;
}

/** null means "best available" for streams and "keep the original" for codecs and container. */
export interface ManualOptions {
  mode: "manual";
  content: Content;
  video_format: string | null;
  audio_format: string | null;
  container: Container | null;
  video_codec: VideoCodec | null;
  audio_codec: AudioCodec | null;
}

export type DownloadOptions = PresetOptions | ManualOptions;

/** The part of the video to keep, in seconds. */
export interface Trim {
  start: number;
  end: number;
  /** Cut on the exact frames (re-encodes) instead of the nearest key frame. */
  exact: boolean;
}

export interface Hint {
  level: "info" | "warning";
  text: string;
}

export interface QualityOption {
  /** Short side in pixels, e.g. 1080. */
  lines: number;
  /** Picking this resolution means converting the video. */
  converts: boolean;
}

export interface PlanSummary {
  headline: string;
  detail: string;
  converts: boolean;
  /** Bytes, when predictable. */
  estimated_size: number | null;
  hints: Hint[];
  qualities: QualityOption[];
  /** The resolution this plan resolved to. */
  lines: number | null;
  ext: string;
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

export interface DownloadRequest {
  id: string;
  url: string;
  options: DownloadOptions;
  trim: Trim | null;
}

export type DownloadEvent =
  | ({ type: "progress"; id: string } & Progress)
  | { type: "finished"; id: string; path: string; size: number | null }
  | { type: "failed"; id: string; error: AppError }
  | { type: "paused"; id: string }
  | { type: "cancelled"; id: string };
