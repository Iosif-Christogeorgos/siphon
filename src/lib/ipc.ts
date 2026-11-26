import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppError,
  Browser,
  Choice,
  Choices,
  DownloadEvent,
  DownloadRequest,
  Intake,
  Measure,
  Media,
  MkvContents,
  AddedFile,
  Chapter,
  PickedFile,
  Placement,
  PlanSummary,
  Preview,
  Recovered,
  Registry,
  Settings,
  SettingsView,
  SignInMethod,
  SignInMode,
  SignInOutcome,
  SignInStatus,
  SiteSession,
  Speed,
  SubSyncStatus,
  Synced,
  SyncProgress,
  ToolStatus,
  Trim,
  UpdateNotice,
  VideoInfo,
} from "@/types";

/** Typed wrappers around every backend command (see `src-tauri/src/commands.rs`). */
export const ipc = {
  binariesStatus: () => invoke<ToolStatus[]>("binaries_status"),
  /** `install` forces updates to be installed even in "ask first" mode. */
  binariesSync: (install: boolean) => invoke<ToolStatus[]>("binaries_sync", { install }),
  /** Without an account unless the video needs one; `account` uses the site's sign-in for this lookup. */
  fetchMedia: (url: string, account = false) => invoke<Media>("fetch_media", { url, account }),
  /** Browse: Windows' file dialog, opened by the backend; null if it was closed. */
  pickFiles: () => invoke<Intake | null>("pick_files"),
  /** What a file the user gave holds, by its key. */
  readFile: (key: string) => invoke<VideoInfo>("read_file", { key }),
  /** Every track of an MKV the user gave, with how certain its language is. */
  readTracks: (key: string) => invoke<MkvContents>("read_tracks", { key }),
  /** The file dialog for subtitles and sound to add to MKVs; null if it was closed. */
  pickTrackFiles: () => invoke<PickedFile[] | null>("pick_track_files"),
  /** A file to add tracks from, read; a text subtitle file in `charset`, if chosen. */
  readAdded: (key: string, charset: string | null = null) => invoke<AddedFile>("read_added", { key, charset }),
  /** Subtitle sync's download, and whether this PC can use it. */
  subsyncStatus: () => invoke<SubSyncStatus>("subsync_status"),
  /** Downloads subtitle sync; progress comes as `subsync://status` events. */
  subsyncInstall: () => invoke<void>("subsync_install"),
  subsyncRemove: () => invoke<void>("subsync_remove"),
  /**
   * Syncs subtitles to the speech in `file`'s stream `stream`: `subs`, a
   * subtitle file added to `file`'s copy, or else `file`'s own text subtitle track `track`.
   */
  syncSubtitles: (
    file: string,
    subs: string | null,
    track: number | null,
    stream: number,
    language: string | null,
    charset: string | null,
  ) => invoke<Synced>("sync_subtitles", { file, subs, track, stream, language, charset }),
  syncCancel: () => invoke<void>("sync_cancel"),
  /** Takes away every synced copy of subtitles but those `keys` name. */
  syncKeep: (keys: string[]) => invoke<void>("sync_keep", { keys }),
  /** The chapters in a chapters file (XML, text) or a video the user gave. */
  readChapters: (key: string) => invoke<Chapter[]>("read_chapters", { key }),
  /** Which episode each file to add goes with, by name: its place in `episodes`, or null. */
  matchEpisodes: (episodes: string[], files: string[]) =>
    invoke<(number | null)[]>("match_episodes", { episodes, files }),
  getPresets: () => invoke<Registry>("get_presets"),
  getChoices: () => invoke<Choices>("get_choices"),
  setChoices: (choices: Choices) => invoke<Choices>("set_choices", { choices }),
  /** Hands over what an older version kept in the webview's storage; only used once. */
  importLegacyChoices: (old: unknown) => invoke<Choices>("import_legacy_choices", { old }),
  /** For a live stream, what recording it does (`fromStart`: from the start), sized per hour. */
  planDownload: (url: string, choice: Choice, trim: Trim | null, fromStart = false) =>
    invoke<PlanSummary>("plan_download", { url, choice, trim, fromStart }),
  /** Measures in the background what the plan could only estimate; the plan says more once it's found. */
  measureSize: (url: string, choice: Choice, trim: Trim | null) =>
    invoke<Measure>("measure_size", { url, choice, trim }),
  /** Ends the size measurement running now, if any. */
  stopMeasuring: () => invoke<void>("stop_measuring"),
  /** Whether a live stream can be recorded from its start (a lookup of a few seconds). */
  liveFromStart: (url: string) => invoke<boolean>("live_from_start", { url }),
  /** Bytes, looking the video up first if needed (for playlist totals). */
  estimateDownload: (url: string, choice: Choice) => invoke<number | null>("estimate_download", { url, choice }),
  previewSource: (url: string, level: number) => invoke<Preview>("preview_source", { url, level }),
  /**
   * The chat box at `at` seconds of the video, placed on a frame `width` × `height`:
   * its left, top, width and height as four little-endian u32s, then its pixels (RGBA).
   */
  chatPreview: (url: string, at: number, placement: Placement, width: number, height: number) =>
    invoke<ArrayBuffer>("chat_preview", { url, at, placement, width, height }),
  startDownload: (request: DownloadRequest) => invoke<void>("start_download", { request }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  /** Ends a recording there, and makes it into its file. */
  stopRecording: (id: string) => invoke<void>("stop_recording", { id }),
  /** Finishes recordings Siphon's closing cut short; once, at launch. */
  recoverRecordings: () => invoke<Recovered[]>("recover_recordings"),
  forgetOtherDownloads: (ids: string[]) => invoke<void>("forget_other_downloads", { ids }),
  getSettings: () => invoke<SettingsView>("get_settings"),
  setSettings: (settings: Settings) => invoke<SettingsView>("set_settings", { settings }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  revealFile: (path: string) => invoke<void>("reveal_file", { path }),
  /** Which of these downloaded files are still there, in the same order. */
  filesPresent: (paths: string[]) => invoke<boolean[]>("files_present", { paths }),
  /** A Windows notification, shown only while Siphon isn't the active window. */
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
  /** Browsers on this computer a sign-in can be taken from. */
  signInBrowsers: () => invoke<Browser[]>("sign_in_browsers"),
  /** Signs in to the site of `url` (a video that needed it), checked on that video. */
  signIn: (url: string, method: SignInMethod) => invoke<SignInOutcome>("sign_in", { url, method }),
  /** The sign-in window's bar: which site it's for. */
  signInSite: () => invoke<string | null>("sign_in_site"),
  signInDone: () => invoke<void>("sign_in_done"),
  signInCancel: () => invoke<void>("sign_in_cancel"),
  signedInSites: () => invoke<SiteSession[]>("signed_in_sites"),
  signOut: (site: string) => invoke<void>("sign_out", { site }),
  setSignInMode: (site: string, mode: SignInMode) => invoke<void>("set_sign_in_mode", { site, mode }),
  /** The version of a newer Siphon release, or null. */
  appUpdateCheck: () => invoke<string | null>("app_update_check"),
  /** Installs the newest release and restarts the app. */
  appUpdateInstall: () => invoke<void>("app_update_install"),
};

export const onToolStatus = (handler: (status: ToolStatus) => void): Promise<UnlistenFn> =>
  listen<ToolStatus>("binaries://status", (event) => handler(event.payload));

export const onToolUpdated = (handler: (notice: UpdateNotice) => void): Promise<UnlistenFn> =>
  listen<UpdateNotice>("binaries://updated", (event) => handler(event.payload));

export const onSubSyncStatus = (handler: (status: SubSyncStatus) => void): Promise<UnlistenFn> =>
  listen<SubSyncStatus>("subsync://status", (event) => handler(event.payload));

export const onSyncProgress = (handler: (progress: SyncProgress) => void): Promise<UnlistenFn> =>
  listen<SyncProgress>("subsync://progress", (event) => handler(event.payload));

export const onDownloadEvent =(handler: (event: DownloadEvent) => void): Promise<UnlistenFn> =>
  listen<DownloadEvent>("download://event", (event) => handler(event.payload));

/** The total speed of all downloads, twice a second while anything downloads, then one zero. */
export const onSpeed = (handler: (speed: Speed) => void): Promise<UnlistenFn> =>
  listen<Speed>("speed://total", (event) => handler(event.payload));

/** The sign-in window's state, for its bar. */
export const onSignInStatus = (handler: (status: SignInStatus) => void): Promise<UnlistenFn> =>
  listen<SignInStatus>("sign-in://status", (event) => handler(event.payload));

/** Files were dropped on the window: what the backend took in from them. */
export const onFilesDropped = (handler: (intake: Intake) => void): Promise<UnlistenFn> =>
  listen<Intake>("files://dropped", (event) => handler(event.payload));

/** Whether a "link" is a file's key: a file on this PC the user gave, not a link. */
export const isFileKey = (url: string) => url.startsWith("siphon-file:");

/** Commands reject with an `AppError`; anything else is wrapped to look like one. */
export function toAppError(error: unknown): AppError {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as AppError;
  }
  return { kind: "internal", message: "Something unexpected went wrong.", details: String(error), sign_in: null };
}
