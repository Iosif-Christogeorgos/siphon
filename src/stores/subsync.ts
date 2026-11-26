import { useEffect, useRef } from "react";
import { create } from "zustand";
import { ipc, toAppError } from "@/lib/ipc";
import { useDownloads } from "@/stores/downloads";
import type { SubSyncStatus, Synced, SyncProgress } from "@/types";

interface SyncRequest {
  /** What names this sync while it runs: the added file's key, or `<film>#<track>` (`ownSyncId`). */
  id: string;
  /** The film's key. */
  file: string;
  /** The added subtitle file's key; or else */
  subs: string | null;
  /** the film's own text subtitle track, by its place among its streams. */
  track: number | null;
  /** The film's sound stream to listen to, and its language tag. */
  stream: number;
  language: string | null;
  charset: string | null;
}

/** A sync asked for: what to sync, and what to do with the synced copy. */
interface SyncJob extends SyncRequest {
  done: (synced: Synced) => void;
}

interface SubSyncState {
  /** Null until the backend has said. */
  status: SubSyncStatus | null;
  /** The sync running now, and how far it has got. */
  running: SyncJob | null;
  progress: SyncProgress | null;
  /** The syncs waiting their turn, the next first. */
  queue: SyncJob[];
  /** Why syncs failed, by id; until they are asked for again. */
  failures: Record<string, string>;
  load: () => Promise<void>;
  apply: (status: SubSyncStatus) => void;
  onProgress: (progress: SyncProgress) => void;
  install: () => Promise<void>;
  remove: () => Promise<string | null>;
  /**
   * Syncs subtitles, now or once those asked for before have been: one at a
   * time, as the models fill most of the graphics card. `done` gets the synced
   * copy (it goes on even if the tracks card closes). Asked for again while
   * waiting or running, nothing more happens.
   */
  sync: (request: SyncRequest, done: (synced: Synced) => void) => void;
  /** Takes these syncs out of the queue, and stops the one running if it is one of them: nothing is applied of it. */
  stop: (ids: string[]) => void;
  /** Stops every sync of these films: they have left the window, and their choices of tracks with them. */
  leave: (files: string[]) => void;
}

/** The id of the sync running now that was stopped: whatever it returns isn't applied. */
let stopped: string | null = null;

/**
 * Subtitle sync's optional download (docs/subtitle-sync.md), and the syncs
 * asked for: one runs at a time, the rest wait their turn in the order asked.
 */
export const useSubSync = create<SubSyncState>((set, get) => {
  /** Starts the next sync waiting, if none runs. */
  const next = async () => {
    const { running, queue } = get();
    const [job, ...rest] = queue;
    if (running || !job) return;
    stopped = null;
    set({ running: job, queue: rest, progress: null });
    try {
      const synced = await ipc.syncSubtitles(job.file, job.subs, job.track, job.stream, job.language, job.charset);
      if (stopped !== job.id) job.done(synced);
    } catch (error) {
      const failed = toAppError(error);
      if (failed.kind !== "cancelled" && stopped !== job.id) {
        set((state) => ({ failures: { ...state.failures, [job.id]: failed.message } }));
      }
    } finally {
      set({ running: null, progress: null });
      // A new synced copy is on the disk: whether anything uses it is told even
      // when what is used hasn't changed (the film was closed, or another opened,
      // while it synced). It is in its draft, if it has one, by the time this looks.
      told = null;
      review();
      void next();
    }
  };

  return {
    status: null,
    running: null,
    progress: null,
    queue: [],
    failures: {},
    load: async () => set({ status: await ipc.subsyncStatus() }),
    apply: (status) => set({ status }),
    onProgress: (progress) => {
      // Only the sync running now is shown; a stopped one may still report as it ends.
      if (progress.key === get().running?.id && stopped !== progress.key) set({ progress });
    },
    install: async () => {
      // Failures arrive as a status of their own; this only waits for the end.
      await ipc.subsyncInstall().catch(() => {});
      set({ status: await ipc.subsyncStatus() });
    },
    remove: async () => {
      try {
        await ipc.subsyncRemove();
        // Nothing waiting can run without it.
        set({ status: await ipc.subsyncStatus(), queue: [] });
        return null;
      } catch (error) {
        return toAppError(error).message;
      }
    },
    sync: (request, done) => {
      const { running, queue, failures } = get();
      // Asked again just after it was stopped, it waits for the stopped one to end.
      if ((running?.id === request.id && stopped !== request.id) || queue.some((job) => job.id === request.id)) return;
      const { [request.id]: _tried, ...others } = failures;
      set({ queue: [...queue, { ...request, done }], failures: others });
      void next();
    },
    stop: (ids) => {
      const { running, queue } = get();
      set({ queue: queue.filter((job) => !ids.includes(job.id)) });
      if (running && ids.includes(running.id)) {
        stopped = running.id;
        set({ progress: null });
        void ipc.syncCancel();
      }
    },
    leave: (files) => {
      const { running, queue } = get();
      get().stop([...queue, ...(running ? [running] : [])].filter((job) => files.includes(job.file)).map((job) => job.id));
    },
  };
});

/** How many syncs of these films are waiting or running. */
export function syncsOf(state: SubSyncState, files: string[]): number {
  const jobs = state.running ? [state.running, ...state.queue] : state.queue;
  return jobs.filter((job) => files.includes(job.file)).length;
}

/**
 * Stops the syncs of these films when they leave the window (the pane that
 * holds their choices of tracks is closed): nothing would take what they make.
 */
export function useSyncsLeaveWith(files: string[]) {
  const latest = useRef(files);
  latest.current = files;
  useEffect(() => () => useSubSync.getState().leave(latest.current), []);
}

// ── Only what is used is kept ─────────────────────────────────────────

/** The synced copies each holder of drafts uses (`useSyncedInUse`), by holder. */
const holders = new Map<string, string[]>();
/** What the backend was last told to keep. */
let told: string | null = null;
let looking: ReturnType<typeof setTimeout> | null = null;

/** A holder of drafts uses these synced copies now: none, once it lets them go. */
export function holdSynced(holder: string, keys: string[]) {
  if (keys.length > 0) holders.set(holder, keys);
  else holders.delete(holder);
  review();
}

/**
 * Tells the backend which synced copies are still used, a moment after
 * anything may have changed that, so it takes the rest away (`sync_keep`):
 * those a draft holds, and those an unfinished copy reads again when it is
 * resumed or tried again. Not while a sync runs: its copy isn't in a draft yet.
 */
function review() {
  if (looking) return;
  looking = setTimeout(() => {
    looking = null;
    if (useSubSync.getState().running) return;
    const keys = new Set([...holders.values()].flat());
    for (const download of Object.values(useDownloads.getState().byId)) {
      if (download.status === "finished") continue;
      for (const track of download.tracks?.tracks ?? []) if (track.from) keys.add(track.from);
    }
    const kept = [...keys].sort();
    const now = kept.join("\n");
    if (now === told) return;
    told = now;
    ipc.syncKeep(kept).catch(() => {
      told = null;
    });
  }, 500);
}

// A copy that finished, or was taken out of Activity, may have been the last to use one.
useDownloads.subscribe(review);
// And as the window starts: if it was reloaded, its drafts went with it.
review();
