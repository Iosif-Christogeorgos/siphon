import { create } from "zustand";
import { ipc, toAppError } from "@/lib/ipc";
import type { AppError, DownloadEvent, Progress, VideoInfo } from "@/types";

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
  start: (video: VideoInfo) => string;
  cancel: (id: string) => void;
  remove: (id: string) => void;
  apply: (event: DownloadEvent) => void;
}

const INITIAL_PROGRESS: Progress = { stage: "preparing", fraction: 0, speed: null, eta: null };

export const useDownloads = create<DownloadsState>((set, get) => {
  const patch = (id: string, changes: Partial<Download>) => {
    const current = get().byId[id];
    if (current) set({ byId: { ...get().byId, [id]: { ...current, ...changes } } });
  };

  return {
    byId: {},

    start: (video) => {
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
        .startDownload({ id, url: video.url, output_dir: null })
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
