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

/** Subtitle sync's download (`subsync/mod.rs`): optional, made only when asked for in Settings. */
export interface SubSyncStatus {
  state: "no_gpu" | "absent" | "installing" | "ready" | "failed";
  step: "tools" | "python" | "libraries" | "models" | null;
  /** 0–1 over the whole download, while installing. */
  progress: number | null;
  error: string | null;
  /** "NVIDIA GeForce RTX 5080 (16 GB)", or why none can be used. */
  gpu: string | null;
  download_bytes: number;
  disk_bytes: number;
}

/** How far a sync has got (`subsync://progress`); `key` is the subtitle file's. */
export interface SyncProgress {
  key: string;
  stage: "audio" | "speech" | "words" | "pairing" | string;
  fraction: number;
}

/** What a sync found. */
export interface SyncSummary {
  lines: number;
  /** Lines given the time of their own words. */
  by_words: number;
  /** Lines no speech could be found for, placed by the lines around them. */
  placed: number;
  /** Where the first of those now start, in seconds. */
  placed_at: number[];
  seconds: number;
}

/** A subtitle file synced to the speech: its synced copy, read as a file to add. */
export interface Synced {
  added: AddedFile;
  summary: SyncSummary;
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
  /** A stream live now, scheduled, or a channel that's offline: recorded rather than downloaded. */
  live: Live | null;
  /** The subtitle tracks it offers, people's first (`ytdlp/subtitles.rs`). */
  subtitles: SubtitleTrack[];
  /** Where its chat comes from, if it can be drawn in: Twitch's VODs and streams (`chat/mod.rs`). */
  chat: ChatSource | null;
  /** A file on this PC to convert, rather than a link; its `url` is then the file's key (`local/mod.rs`). */
  file?: LocalFile;
}

// ── local/mod.rs ────────────────────────────────────────────────────────

/**
 * A file on this PC the user gave Siphon (dropped, or picked with Browse).
 * The backend keeps its path; the window knows it by its key, which stands
 * where a link would, and never names a file itself.
 */
export interface LocalFile {
  /** "siphon-file:…" */
  key: string;
  /** "Clip.mp4" */
  name: string;
  /** The folder it is in, where what is made from it goes. */
  folder: string;
  /** Bytes. */
  size: number;
  /** Its size and when it last changed: a file changed since is read and planned again. */
  version: string;
  /** A Matroska file inside, whatever its name says: its tracks can be chosen. Known once it is read. */
  matroska: boolean;
  /** Its tracks can be chosen: an MKV, an MP4 or MOV, an MPEG-TS. */
  editable: boolean;
  /** How many video (covers aside), sound and subtitle tracks it holds, and their languages as tagged. */
  overview: TrackOverview;
}

/** A file carried in an MKV: a font for its ASS subtitles, a cover. */
export interface Attachment {
  /** Its place among the file's streams. */
  index: number;
  name: string;
  mime: string | null;
}

/** What a file holds, track by track, as its tags say (`local/mod.rs`). */
export interface TrackOverview {
  video: number;
  sound: number;
  subtitles: number;
  /** Each once, in the file's order: "el". */
  sound_languages: string[];
  subtitle_languages: string[];
}

// ── local/mkv/mod.rs ────────────────────────────────────────────────────

/** What an MKV holds, track by track (docs/mkv-editor.md). */
export interface MkvContents {
  /** The kind of file: an MP4's copy stays an MP4 unless asked, an MPEG-TS's becomes an MKV. */
  container: "matroska" | "mp4" | "ts" | "other";
  tracks: MkvTrack[];
  chapters: MkvChapters;
  /** Files carried in it: fonts for its ASS subtitles, a cover. */
  attachments: Attachment[];
  /** What casts doubt on the file as a whole. */
  doubts: string[];
  /** What it keeps when the user's languages choose: applied as soon as it opens. */
  suggestion: Suggestion | null;
}

/** What keeping the user's languages makes of an MKV (`local/mkv/pick.rs`). */
export interface Suggestion {
  /** The languages it went by, best first. */
  languages: string[];
  edit: TrackEdit;
  /** The tracks it leaves out, and why: "Italian sound: not one of your languages." */
  left_out: { index: number; why: string }[];
  /** What else it changed or kept, a line each. */
  notes: string[];
}

export type MkvKind = "video" | "sound" | "subtitles";

export interface MkvTrack {
  /** Its place among the file's streams, as FFmpeg counts them. */
  index: number;
  kind: MkvKind;
  /** ffprobe's codec name: "hevc", "truehd", "subrip", "hdmv_pgs_subtitle". */
  codec: string;
  /** The name the file gives it: "Surround". */
  name: string | null;
  /** Its language as a tag ("el"); null when unknown. */
  language: string | null;
  default: boolean;
  forced: boolean;
  /** Subtitles for the deaf and hard of hearing. */
  sdh: boolean;
  commentary: boolean;
  /** Sound describing the picture, for the blind. */
  described: boolean;
  original: boolean;
  channels: number | null;
  width: number | null;
  height: number | null;
  /** Lines, packets or frames it holds, where the file says. */
  entries: number | null;
  /** How many bytes it takes, where the file says. */
  bytes: number | null;
  /** What casts doubt on its language: it is kept, never left out for it. */
  doubts: string[];
  /** What reading it found, besides: a tag put right, SDH, forced. */
  notes: string[];
  /** Its language was found, or its tag borne out, by reading its words. */
  read: boolean;
  /** Its language is known and nothing casts doubt on it (always, for video). */
  certain: boolean;
}

/** The tracks an MKV keeps, and how (`local/mkv/edit.rs`). */
export interface TrackEdit {
  /** The tracks kept, in the order they go in the copy. */
  tracks: KeptTrack[];
  chapters: boolean;
  attachments: boolean;
  /** The chapters the copy gets, as chosen; none: as the MKV has them. */
  chapter_list?: Chapter[] | null;
  /** The MKV's attached files left out, by their place among its streams. */
  left_out_attachments?: number[];
  /** Font files to attach, by key. */
  fonts?: string[];
  /** Save an MP4 as an MKV. */
  mkv?: boolean;
}

/** A chapter: where it starts, and its name. It runs until the next starts. */
export interface Chapter {
  /** Seconds. */
  start: number;
  title: string;
}

export interface KeptTrack {
  /** Its place among its file's streams. */
  index: number;
  /** The file it is added from, by key; none for the MKV's own (`local/mkv/added.rs`). */
  from?: string | null;
  /** For a text subtitle file not in UTF-8: the encoding it is read in ("CP1253"). */
  charset?: string | null;
  /** The language the copy tags it with; null leaves the tag as it is. */
  language: string | null;
  /** Its name in the copy; empty for none. */
  name: string;
  default: boolean;
  forced: boolean;
  sdh: boolean;
  commentary: boolean;
  described: boolean;
  original: boolean;
}

/** A file whose tracks may be added to an MKV's copy, as read (`local/mkv/added.rs`). */
export interface AddedFile {
  key: string;
  /** "Show S01E03.el.srt" */
  name: string;
  contents: MkvContents;
  /** For a text subtitle file not in UTF-8: the encoding it was read in. */
  charset: string | null;
  /** A few of its subtitles' lines, to see that they read right. */
  sample: string[];
  /** Bytes. */
  size: number;
}

/** A file picked to add tracks from. */
export interface PickedFile {
  key: string;
  name: string;
}

export interface MkvChapters {
  /** The chapters a player lists. */
  count: number;
  /** Those chapters. */
  list: Chapter[];
  /** Said when a copy would flatten them: other versions, hidden ones… */
  note: string | null;
}

/** What came of files dropped on the window or picked with Browse. */
export interface Intake {
  files: LocalFile[];
  /** Files in dropped folders that aren't video or audio, left out. */
  left_out: number;
  /** Files past the most Siphon takes at once, left out. */
  over: number;
}

/** A Twitch VOD's chat replay, or a channel's chat as it airs. */
export type ChatSource = { kind: "vod"; id: string; channel: string } | { kind: "live"; channel: string };

/** Where the chat goes on the picture, as shares of the frame (0–1), and how dark its box is (0–1). */
export interface Placement {
  x: number;
  y: number;
  width: number;
  height: number;
  shade: number;
}

/** Who made a subtitle track: people, or speech recognition. */
export type SubtitleKind = "uploaded" | "automatic";

export interface SubtitleTrack {
  /** yt-dlp's name for it: "en", "de-DE", "en-orig". */
  key: string;
  /** Its language as a lowercase tag: "en", "de-de", "pt-br". */
  lang: string;
  /** The site's name for it, if it gives one. */
  name: string | null;
  kind: SubtitleKind;
}

/** Where a live stream stands (`metadata::LiveStatus`). */
export type LiveStatus = "live" | "upcoming" | "offline";

export interface Live {
  status: LiveStatus;
  /** Unix seconds: when it went on air (live), or is to (upcoming), if the site says. */
  since: number | null;
  /** How many are watching, if the site says. */
  viewers: number | null;
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
  /** A stream live now or scheduled: opened to record, rather than queued. */
  live: Exclude<LiveStatus, "offline"> | null;
}

export interface PlaylistInfo {
  id: string;
  title: string;
  url: string;
  channel: string | null;
  entries: PlaylistEntry[];
  /** A Twitch channel's videos: the channel (its login), whose chat they can be saved with. */
  chat_channel: string | null;
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

export type VideoCodec = "h264" | "hevc" | "vp9" | "av1" | "vp8" | "other";
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
export type Extra = "tier" | "music_format";

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
      /** The language subtitles are wanted in ("en", "pt-br"); null is none. */
      subtitles: string | null;
      /** Draw the chat into the picture, here; null is without chat. */
      chat: Placement | null;
    }
  | ({ mode: "custom"; subtitles: string | null } & ManualOptions);

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
  /** How subtitles would reach the file with this choice, in any language; null for audio, which can take none. */
  subtitle_delivery: SubtitleDelivery | null;
  /** The plan draws the video's chat into the picture. */
  chat: boolean;
  /** The subtitle track the plan adds (its key), if any. */
  subtitle_track: string | null;
  /** Measuring the video would tell its size better: the window asks for it (`useMeasuredSize`). */
  measurable: boolean;
  /** The size of this conversion was measured on the video itself. */
  measured: boolean;
  /** A file that already is what was asked for: why, and that nothing is made of it. */
  nothing: string | null;
}

/** How a size measurement went (`sizing::Measure`). */
export type Measure = "found" | "nothing" | "stopped";

/** Inside the file as a track, or drawn into the picture (`plan/subtitles.rs`). */
export type SubtitleDelivery = "embed" | "burn";

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
  custom: ManualOptions;
  saved: SavedPreset[];
  /** The language subtitles are wanted in, for every video that has it; null is none. */
  subtitles: string | null;
  /** Draw a Twitch video's chat into the picture. */
  chat: boolean;
  /** Where the chat goes, by channel (Twitch's login). */
  chat_places: Record<string, Placement>;
}

// ── settings.rs ────────────────────────────────────────────────────────

export type FilenameStyle = "title" | "title_id" | "channel_title" | "date_title";

export interface Settings {
  /** Where downloads are saved; null means the system Downloads folder. */
  output_dir: string | null;
  filename: FilenameStyle;
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
  /** The languages the user listens and reads in, best first ("el", "en"); empty until chosen. */
  languages: string[];
}

/** What `get_settings` returns: the settings plus facts for displaying them. */
export interface SettingsView extends Settings {
  default_output_dir: string | null;
  /** The languages taken as the user's until they choose some: their subtitle language, then Windows'. */
  default_languages: string[];
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
  /** Fetching subtitles after the video, or adding them to copied streams. */
  | "subtitles"
  /** Fetching the chat after the video. */
  | "chat"
  /** Encoding the video with its chat drawn in. */
  | "drawing_chat"
  | "finishing"
  /** A recording waits for its stream to start. */
  | "waiting_live"
  /** A recording is under way (or, from the start, catching up). */
  | "recording"
  /** The stream stopped sending; the recording waits a few minutes for it to come back. */
  | "reconnecting"
  /** The recording ended and is being made into its file. */
  | "finalizing"
  /** The tracks of an MKV that were chosen are being copied into a new file. */
  | "copying_tracks";

export interface Progress {
  stage: Stage;
  /** Overall completion, 0–1. */
  fraction: number;
  /** Bytes per second. */
  speed: number | null;
  /** Seconds. */
  eta: number | null;
  /** A recording's own readings, in place of a fraction and time left. */
  live?: LiveProgress;
  /** While Siphon converts: the size the file is heading for, in bytes, from what it has written so far. */
  heading_for?: number;
}

/** How a recording is going (`tracker::LiveProgress`). */
export interface LiveProgress {
  /** Seconds since recording began. */
  elapsed: number;
  /** Bytes recorded so far. */
  bytes: number;
  /** Bits per second, lately. */
  bitrate: number | null;
  /** From the start: seconds behind live, until it catches up. */
  behind: number | null;
  /** Waiting: when the stream is scheduled to start (Unix seconds). */
  starts_at: number | null;
  /** Seconds of recording the disk still has room for, when that's getting short. */
  disk_left: number | null;
  /** Stopped while read from Twitch's VOD, behind live: seconds it still records to get to where Stop was pressed. */
  stop_in: number | null;
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
  /** Record a live stream instead, waiting for it to start if it hasn't. */
  record: RecordRequest | null;
  /** An MKV on this PC: keep these of its tracks instead of converting it. */
  tracks: TrackEdit | null;
}

export interface RecordRequest {
  /** Record what already aired too (where the site keeps it). */
  from_start: boolean;
}

/** A recording cut short by Siphon closing, finished when it started again (`live::Recovered`). */
export interface Recovered {
  id: string;
  url: string;
  title: string;
  thumbnail: string | null;
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
  | {
      type: "finished";
      id: string;
      path: string;
      size: number | null;
      /** Something worth saying about the file: why a recording was kept as recorded, or that the disk filled up. */
      note?: string;
      /** A recording kept as it was recorded, not made into the preset's file. */
      as_recorded?: boolean;
      /** `note` in a word or two, for the download list ("No subtitles"). */
      note_label?: string;
      /** A recording Siphon's closing cut short, finished once it started again. */
      recovered?: boolean;
    }
  | { type: "failed"; id: string; error: AppError }
  | { type: "paused"; id: string }
  | { type: "cancelled"; id: string };
