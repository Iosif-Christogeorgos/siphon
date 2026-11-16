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
}

// ── ytdlp/metadata.rs ───────────────────────────────────────────────────

export type FormatKind = "video" | "audio" | "muxed";

export interface Format {
  id: string;
  ext: string;
  kind: FormatKind;
  vcodec: string | null;
  acodec: string | null;
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

// ── downloads ───────────────────────────────────────────────────────────

export type Stage =
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
  output_dir: string | null;
}

export type DownloadEvent =
  | ({ type: "progress"; id: string } & Progress)
  | { type: "finished"; id: string; path: string; size: number | null }
  | { type: "failed"; id: string; error: AppError }
  | { type: "cancelled"; id: string };
