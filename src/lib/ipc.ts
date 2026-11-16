import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppError,
  DownloadEvent,
  DownloadOptions,
  DownloadRequest,
  Media,
  PlanSummary,
  Preview,
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
  planDownload: (url: string, options: DownloadOptions, trim: Trim | null) =>
    invoke<PlanSummary>("plan_download", { url, options, trim }),
  previewSource: (url: string, level: number) => invoke<Preview>("preview_source", { url, level }),
  startDownload: (request: DownloadRequest) => invoke<void>("start_download", { request }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  pauseDownload: (id: string) => invoke<void>("pause_download", { id }),
  getSettings: () => invoke<SettingsView>("get_settings"),
  setSettings: (settings: Settings) => invoke<SettingsView>("set_settings", { settings }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  revealFile: (path: string) => invoke<void>("reveal_file", { path }),
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
