import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppError, DownloadEvent, DownloadRequest, Media, ToolStatus } from "@/types";

/** Typed wrappers around every backend command (see `src-tauri/src/commands.rs`). */
export const ipc = {
  binariesStatus: () => invoke<ToolStatus[]>("binaries_status"),
  binariesSync: () => invoke<ToolStatus[]>("binaries_sync"),
  fetchMedia: (url: string) => invoke<Media>("fetch_media", { url }),
  startDownload: (request: DownloadRequest) => invoke<void>("start_download", { request }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  revealFile: (path: string) => invoke<void>("reveal_file", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
};

export const onToolStatus = (handler: (status: ToolStatus) => void): Promise<UnlistenFn> =>
  listen<ToolStatus>("binaries://status", (event) => handler(event.payload));

export const onDownloadEvent = (handler: (event: DownloadEvent) => void): Promise<UnlistenFn> =>
  listen<DownloadEvent>("download://event", (event) => handler(event.payload));

/** Commands reject with an `AppError`; anything else is wrapped to look like one. */
export function toAppError(error: unknown): AppError {
  if (typeof error === "object" && error !== null && "kind" in error && "message" in error) {
    return error as AppError;
  }
  return { kind: "internal", message: "Something unexpected went wrong.", details: String(error) };
}
