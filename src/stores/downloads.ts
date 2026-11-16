import { useMemo } from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { ipc, toAppError } from "@/lib/ipc";
import type { AppError, DownloadEvent, DownloadOptions, Progress, Trim, VideoInfo } from "@/types";

export type DownloadStatus = "running" | "finished" | "failed";

export interface Download {
  id: string;
  url: string;
  title: string;
  status: DownloadStatus;
  progress: Progress;
  /** Set once finished. */
  path: string | null;
  size: number | null;
  /** Set once failed. */
  error: AppError | null;
}

interface DownloadsState {
  byId: Record<string, Download>;
  /** Starts downloading a video and returns the new download's id. */
  start: (video: VideoInfo, options: DownloadOptions, trim: Trim | null) => string;
  cancel: (id: string) => void;
  remove: (id: string) => void;
  apply: (event: DownloadEvent) => void;
}

/** Several downloads started together (one per trimmed part), seen as one. */
export interface DownloadGroup extends Download {
  ids: string[];
  /** How many downloads the group holds. */
  count: number;
}

/**
 * Folds the downloads with these ids into one status: running while any
 * runs, failed if one failed, finished when all are.
 */
export function useDownloadGroup(ids: string[]): DownloadGroup | undefined {
  const downloads = useDownloads(
    useShallow((state) => ids.map((id) => state.byId[id]).filter((download) => download !== undefined)),
  );
  return useMemo(() => {
    const first = downloads[0];
    if (!first) return undefined;
    const running = downloads.filter((download) => download.status === "running");
    const failed = downloads.find((download) => download.status === "failed");
    const lead = running[0] ?? failed ?? first;
    const sizes = downloads.map((download) => download.size);
    return {
      ...lead,
      ids: downloads.map((download) => download.id),
      count: downloads.length,
      status: running.length > 0 ? "running" : failed ? "failed" : "finished",
      progress: {
        ...lead.progress,
        fraction: downloads.reduce((sum, download) => sum + download.progress.fraction, 0) / downloads.length,
      },
      path: first.path,
      size: sizes.every((size) => size !== null) ? sizes.reduce((sum, size) => sum + size, 0) : null,
      error: failed?.error ?? null,
    };
  }, [downloads]);
}

const INITIAL_PROGRESS: Progress = { stage: "preparing", fraction: 0, speed: null, eta: null };

export const useDownloads = create<DownloadsState>((set, get) => {
  const patch = (id: string, changes: Partial<Download>) => {
    const current = get().byId[id];
    if (current) set({ byId: { ...get().byId, [id]: { ...current, ...changes } } });
  };

  return {
    byId: {},

    start: (video, options, trim) => {
      const id = crypto.randomUUID();
      const download: Download = {
        id,
        url: video.url,
        title: video.title,
        status: "running",
        progress: INITIAL_PROGRESS,
        path: null,
        size: null,
        error: null,
      };
      set({ byId: { ...get().byId, [id]: download } });
      ipc
        .startDownload({ id, url: video.url, options, trim, output_dir: null })
        .catch((error) => patch(id, { status: "failed", error: toAppError(error) }));
      return id;
    },

    cancel: (id) => {
      void ipc.cancelDownload(id);
    },

    remove: (id) => {
      const { [id]: _removed, ...rest } = get().byId;
      set({ byId: rest });
    },

    apply: (event) => {
      switch (event.type) {
        case "progress": {
          const { type: _type, id, ...progress } = event;
          patch(id, { progress });
          break;
        }
        case "finished":
          patch(event.id, {
            status: "finished",
            path: event.path,
            size: event.size,
            progress: { stage: "finishing", fraction: 1, speed: null, eta: null },
          });
          break;
        case "failed":
          patch(event.id, { status: "failed", error: event.error });
          break;
        case "cancelled":
          get().remove(event.id);
          break;
      }
    },
  };
});
