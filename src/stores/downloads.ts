import { useMemo } from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { ipc, toAppError } from "@/lib/ipc";
import type { AppError, DownloadEvent, DownloadOptions, Progress, Trim } from "@/types";

export type DownloadStatus = "queued" | "running" | "paused" | "finished" | "failed";

/** What a download is of: enough to start it and to show it in the queue. */
export interface DownloadSource {
  url: string;
  title: string;
  thumbnail: string | null;
}

export interface Download extends DownloadSource {
  id: string;
  options: DownloadOptions;
  trim: Trim | null;
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
  /** Ids in the order they were added. */
  order: string[];
  /** Adds a download to the queue and returns its id. */
  start: (source: DownloadSource, options: DownloadOptions, trim: Trim | null) => string;
  /** Stops a download but keeps what it has fetched. */
  pause: (id: string) => void;
  /** Continues a paused download where it stopped. */
  resume: (id: string) => void;
  /** Starts a failed download again; it carries on from what it had fetched. */
  retry: (id: string) => void;
  /** Takes a download off the list, stopping it and deleting partial files if needed. */
  remove: (id: string) => void;
  /** Takes every finished download off the list (the files stay). */
  clearFinished: () => void;
  apply: (event: DownloadEvent) => void;
}

const WAITING: Progress = { stage: "queued", fraction: 0, speed: null, eta: null };

export const isActive = (download: Download) => download.status === "queued" || download.status === "running";

export const useDownloads = create<DownloadsState>((set, get) => {
  const patch = (id: string, changes: Partial<Download>) => {
    const current = get().byId[id];
    if (current) set({ byId: { ...get().byId, [id]: { ...current, ...changes } } });
  };
  const drop = (ids: string[]) => {
    const gone = new Set(ids);
    set({
      byId: Object.fromEntries(Object.entries(get().byId).filter(([id]) => !gone.has(id))),
      order: get().order.filter((id) => !gone.has(id)),
    });
  };
  /**
   * Hands a download to the backend. Under an id it has seen before, the
   * backend carries on from the files that download left when it was paused
   * or failed.
   */
  const submit = (download: Download) => {
    const { id, url, options, trim } = download;
    ipc
      .startDownload({ id, url, options, trim })
      .catch((error) => patch(id, { status: "failed", error: toAppError(error) }));
  };

  return {
    byId: {},
    order: [],

    start: (source, options, trim) => {
      const download: Download = {
        ...source,
        id: crypto.randomUUID(),
        options,
        trim,
        status: "queued",
        progress: WAITING,
        path: null,
        size: null,
        error: null,
      };
      set({ byId: { ...get().byId, [download.id]: download }, order: [...get().order, download.id] });
      submit(download);
      return download.id;
    },

    pause: (id) => void ipc.pauseDownload(id),

    resume: (id) => {
      const download = get().byId[id];
      if (download?.status !== "paused") return;
      // Keep the fraction it reached, so the bar doesn't jump back while it waits.
      patch(id, { status: "queued", progress: { ...download.progress, stage: "queued", speed: null, eta: null } });
      submit(download);
    },

    retry: (id) => {
      const download = get().byId[id];
      if (download?.status !== "failed") return;
      patch(id, {
        status: "queued",
        progress: { ...download.progress, stage: "queued", speed: null, eta: null },
        error: null,
      });
      submit(download);
    },

    remove: (id) => {
      const download = get().byId[id];
      if (!download) return;
      // The backend stops it if it runs, and deletes what a paused or failed one left behind.
      if (download.status !== "finished") void ipc.cancelDownload(id);
      drop([id]);
    },

    clearFinished: () => drop(get().order.filter((id) => get().byId[id]?.status === "finished")),

    apply: (event) => {
      switch (event.type) {
        case "progress": {
          const { type: _type, id, ...progress } = event;
          // A resumed download reports from zero until yt-dlp finds its partial
          // file; the bar stays where it was instead of jumping back.
          const reached = get().byId[id]?.progress.fraction ?? 0;
          patch(id, {
            status: progress.stage === "queued" ? "queued" : "running",
            progress: { ...progress, fraction: Math.max(progress.fraction, reached) },
          });
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
        case "paused":
          patch(event.id, { status: "paused" });
          break;
        case "cancelled":
          drop([event.id]);
          break;
      }
    },
  };
});

/** Several downloads started together (one per trimmed part), seen as one. */
export interface DownloadGroup extends Download {
  ids: string[];
  /** How many downloads the group holds. */
  count: number;
}

/**
 * Folds the downloads with these ids into one status. In order of priority:
 * running while any runs, queued while any waits, then failed, paused, finished.
 */
export function useDownloadGroup(ids: string[]): DownloadGroup | undefined {
  const downloads = useDownloads(
    useShallow((state) => ids.map((id) => state.byId[id]).filter((download) => download !== undefined)),
  );
  return useMemo(() => {
    const first = downloads[0];
    if (!first) return undefined;
    const find = (status: DownloadStatus) => downloads.find((download) => download.status === status);
    const lead = find("running") ?? find("queued") ?? find("failed") ?? find("paused") ?? first;
    const sizes = downloads.map((download) => download.size);
    return {
      ...lead,
      ids: downloads.map((download) => download.id),
      count: downloads.length,
      progress: {
        ...lead.progress,
        fraction: downloads.reduce((sum, download) => sum + download.progress.fraction, 0) / downloads.length,
      },
      path: first.path,
      size: sizes.every((size) => size !== null) ? sizes.reduce((sum, size) => sum + size, 0) : null,
      error: find("failed")?.error ?? null,
    };
  }, [downloads]);
}

/** How many downloads are waiting or running, for the title-bar badge. */
export const selectActiveCount = (state: DownloadsState) =>
  state.order.reduce((count, id) => count + (state.byId[id] && isActive(state.byId[id]) ? 1 : 0), 0);
