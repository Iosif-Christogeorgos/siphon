import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppError,
  Choice,
  Choices,
  DownloadEvent,
  DownloadRequest,
  Media,
  PlanSummary,
  Preview,
  Registry,
  Settings,
  SettingsView,
  ToolStatus,
  Trim,
  UpdateNotice,
} from "@/types";

/** Typed wrappers around every backend command (see `src-tauri/src/commands.rs`). */
export const ipc = {
  binariesStatus: () => invoke<ToolStatus[]>("binaries_status"),
  /** `install` forces updates to be installed even in "ask first" mode. */
  binariesSync: (install: boolean) => invoke<ToolStatus[]>("binaries_sync", { install }),
  fetchMedia: (url: string) => invoke<Media>("fetch_media", { url }),
  getPresets: () => invoke<Registry>("get_presets"),
  getChoices: () => invoke<Choices>("get_choices"),
  setChoices: (choices: Choices) => invoke<Choices>("set_choices", { choices }),
  /** Hands over what an older version kept in the webview's storage; only used once. */
  importLegacyChoices: (old: unknown) => invoke<Choices>("import_legacy_choices", { old }),
  planDownload: (url: string, choice: Choice, trim: Trim | null) =>
    invoke<PlanSummary>("plan_download", { url, choice, trim }),
  /** Bytes, looking the video up first if needed (for playlist totals). */
  estimateDownload: (url: string, choice: Choice) => invoke<number | null>("estimate_download", { url, choice }),
  previewSource: (url: string, level: number) => invoke<Preview>("preview_source", { url, level }),
  startDownload: (request: DownloadRequest) => invoke<void>("start_download", { request }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  getSettings: () => invoke<SettingsView>("get_settings"),
  setSettings: (settings: Settings) => invoke<SettingsView>("set_settings", { settings }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  revealFile: (path: string) => invoke<void>("reveal_file", { path }),
  /** The version of a newer Siphon release, or null. */
  appUpdateCheck: () => invoke<string | null>("app_update_check"),
  /** Installs the newest release and restarts the app. */
  appUpdateInstall: () => invoke<void>("app_update_install"),
};

export const onToolStatus = (handler: (status: ToolStatus) => void): Promise<UnlistenFn> =>
  listen<ToolStatus>("binaries://status", (event) => handler(event.payload));

export const onToolUpdated = (handler: (notice: UpdateNotice) => void): Promise<UnlistenFn> =>
  listen<UpdateNotice>("binaries://updated", (event) => handler(event.payload));

export const onDownloadEvent = (handler: (event: DownloadEvent) => void): Promise<UnlistenFn> =>
  listen<DownloadEvent>("download://event", (event) => handler(event.payload));

/** Commands reject with an `AppError`; anything else is wrapped to look like one. */
export function toAppError(error: unknown): AppError {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as AppError;
  }
  return { kind: "internal", message: "Something unexpected went wrong.", details: String(error) };
}
