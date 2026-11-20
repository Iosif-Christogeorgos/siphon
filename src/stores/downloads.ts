import { useMemo } from "react";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { ipc, toAppError } from "@/lib/ipc";
import type { AppError, Choice, DownloadEvent, Place, Progress, RecordRequest, Trim } from "@/types";

export type DownloadStatus = "queued" | "running" | "paused" | "finished" | "failed";

/** What a download is of: enough to start it and to show it in the queue. */
export interface DownloadSource {
  url: string;
  title: string;
  thumbnail: string | null;
  /** The video was found with the site's sign-in: the download uses it too. */
  with_account?: boolean;
}

export interface Download extends DownloadSource {
  id: string;
  choice: Choice;
  trim: Trim | null;
  /** A playlist video's folder and number, sent again on every resume. Missing on downloads from before 0.11. */
  place?: Place | null;
  status: DownloadStatus;
  progress: Progress;
  /** Set once finished. */
  path: string | null;
  size: number | null;
  /** When it finished (ms since 1970). Missing on downloads finished before 0.11. */
  finishedAt?: number;
  /** Set once failed. */
  error: AppError | null;
  /** Download with the site's sign-in (its video needed it, or the user asked). */
  account?: boolean;
  /** A live recording: records the stream (waiting for it to start) instead of downloading a file. */
  record?: RecordRequest | null;
  /** Something worth saying about the finished file (why a recording was kept as recorded, or that the disk filled up). */
  note?: string;
  /** A recording kept as it was recorded, not made into the preset's file. Missing on recordings finished before 0.13.1. */
  asRecorded?: boolean;
  /** A recording Siphon's closing cut short, finished once it started again. */
  recovered?: boolean;
}

interface DownloadsState {
  byId: Record<string, Download>;
  /** Ids in the order they were added. */
  order: string[];
  /** Adds a download to the queue (a recording starts at once) and returns its id. */
  start: (
    source: DownloadSource,
    choice: Choice,
    trim: Trim | null,
    place?: Place | null,
    record?: RecordRequest | null,
  ) => string;
  /** Ends a recording there and makes it into its file. */
  stop: (id: string) => void;
  /** Shows the recordings the backend is finishing after Siphon closed mid-recording. */
  recover: () => Promise<void>;
  /** Stops a download but keeps what it has fetched. */
  pause: (id: string) => void;
  /** Continues a paused download where it stopped. */
  resume: (id: string) => void;
  /** Starts a failed download again; it carries on from what it had fetched. `account` uses the site's sign-in from now on. */
  retry: (id: string, account?: boolean) => void;
  /** Takes a download off the list, stopping it and deleting partial files if needed. */
  remove: (id: string) => void;
  /** Takes every finished download off the list (the files stay). */
  clearFinished: () => void;
  /** Resumes every paused download. */
  resumeAll: () => void;
  apply: (event: DownloadEvent) => void;
}

const WAITING: Progress = { stage: "queued", fraction: 0, speed: null, eta: null };

/** Where the list is kept between sessions (the backend keeps the files; see `forget_other_downloads`). */
const STORAGE_KEY = "siphon.downloads";
/** The most finished downloads kept in the list; older ones drop off. */
const MAX_FINISHED = 200;

interface SavedList {
  byId: Record<string, Download>;
  order: string[];
}

/**
 * The list from the last session. Downloads that were running or waiting
 * when Siphon closed were paused by it (their files are kept), so they come
 * back paused, ready to resume.
 */
function restore(): SavedList {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as SavedList | null;
    if (!saved || !Array.isArray(saved.order) || typeof saved.byId !== "object") return { byId: {}, order: [] };
    const order = saved.order.filter((id) => saved.byId[id]?.id === id);
    const byId: Record<string, Download> = {};
    for (const id of order) {
      const download = saved.byId[id] as Download;
      byId[id] = isActive(download)
        ? { ...download, status: "paused", progress: { ...download.progress, speed: null, eta: null } }
        : download;
    }
    return { byId, order };
  } catch {
    return { byId: {}, order: [] };
  }
}

/** Writes the list, keeping only the newest finished downloads. */
function persist({ byId, order }: SavedList) {
  const finished = order.filter((id) => byId[id]?.status === "finished");
  const dropped = new Set(finished.slice(0, Math.max(0, finished.length - MAX_FINISHED)));
  const kept = order.filter((id) => !dropped.has(id));
  try {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ order: kept, byId: Object.fromEntries(kept.map((id) => [id, byId[id]])) }),
    );
  } catch {
    // Storage full or unavailable: the list just won't outlive this session.
  }
}

export const isActive = (download: Download) => download.status === "queued" || download.status === "running";

/**
 * How downloads ended while `recover` waited for the backend's answer. A
 * recording with nothing in it fails at once, often before that answer
 * arrives, and marking it as being finished afterwards would leave it so
 * for good. They are applied again once the recovered rows are in place.
 */
let endedWhileRecovering: DownloadEvent[] | null = null;

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
    const { id, url, choice, trim, place = null, account = false, record = null } = download;
    ipc
      .startDownload({ id, url, choice, trim, place, account, record })
      .catch((error) => patch(id, { status: "failed", error: toAppError(error) }));
  };

  return {
    ...restore(),

    start: (source, choice, trim, place = null, record = null) => {
      const download: Download = {
        ...source,
        id: crypto.randomUUID(),
        choice,
        trim,
        place,
        record,
        status: "queued",
        progress: WAITING,
        path: null,
        size: null,
        error: null,
        account: source.with_account ?? false,
      };
      set({ byId: { ...get().byId, [download.id]: download }, order: [...get().order, download.id] });
      submit(download);
      return download.id;
    },

    pause: (id) => void ipc.pauseDownload(id),

    stop: (id) => void ipc.stopRecording(id),

    recover: async () => {
      endedWhileRecovering = [];
      const recovered = await ipc.recoverRecordings().catch(() => []);
      const ended = endedWhileRecovering;
      endedWhileRecovering = null;
      for (const recording of recovered) {
        const progress: Progress = { stage: "finalizing", fraction: 0, speed: null, eta: null };
        const known = get().byId[recording.id];
        if (known) {
          patch(recording.id, { status: "running", progress, recovered: true });
          continue;
        }
        // The list lost it (cleared storage): it comes back so its file isn't a surprise.
        const download: Download = {
          id: recording.id,
          url: recording.url,
          title: recording.title,
          thumbnail: recording.thumbnail,
          choice: { mode: "preset", target: "max", quality: null, tier: null, music_format: "mp3", smooth_editing: false },
          trim: null,
          record: { from_start: false },
          status: "running",
          progress,
          path: null,
          size: null,
          error: null,
          recovered: true,
        };
        set({ byId: { ...get().byId, [download.id]: download }, order: [...get().order, download.id] });
      }
      const ids = new Set(recovered.map((recording) => recording.id));
      for (const event of ended) {
        if (ids.has(event.id)) get().apply(event);
      }
    },

    resume: (id) => {
      const download = get().byId[id];
      if (download?.status !== "paused") return;
      // Keep the fraction it reached, so the bar doesn't jump back while it waits.
      patch(id, { status: "queued", progress: { ...download.progress, stage: "queued", speed: null, eta: null } });
      submit(download);
    },

    retry: (id, account) => {
      const download = get().byId[id];
      if (download?.status !== "failed") return;
      const next = { ...download, account: download.account || account === true };
      patch(id, {
        status: "queued",
        progress: { ...download.progress, stage: "queued", speed: null, eta: null },
        error: null,
        account: next.account,
      });
      submit(next);
    },

    remove: (id) => {
      const download = get().byId[id];
      if (!download) return;
      // The backend stops it if it runs, and deletes what a paused or failed one left behind.
      if (download.status !== "finished") void ipc.cancelDownload(id);
      drop([id]);
    },

    clearFinished: () => drop(get().order.filter((id) => get().byId[id]?.status === "finished")),

    resumeAll: () => {
      for (const id of get().order) get().resume(id);
    },

    apply: (event) => {
      if (endedWhileRecovering && event.type !== "progress") endedWhileRecovering.push(event);
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
            note: event.note,
            asRecorded: event.as_recorded === true,
            recovered: event.recovered || get().byId[event.id]?.recovered,
            finishedAt: Date.now(),
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

// Saved a moment after each change (progress changes often), and at once when asked.
let saveTimer: number | undefined;
export function saveDownloadsNow() {
  window.clearTimeout(saveTimer);
  persist(useDownloads.getState());
}
useDownloads.subscribe(() => {
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(saveDownloadsNow, 1000);
});
window.addEventListener("pagehide", saveDownloadsNow);

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
