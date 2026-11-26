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

interface SubSyncState {
  /** Null until the backend has said. */
  status: SubSyncStatus | null;
  /** The subtitles being synced now, by their `syncId`, and how far they have got. */
  running: string | null;
  progress: SyncProgress | null;
  /** Why the last sync of a file failed. */
  failure: { key: string; message: string } | null;
  load: () => Promise<void>;
  apply: (status: SubSyncStatus) => void;
  onProgress: (progress: SyncProgress) => void;
  install: () => Promise<void>;
  remove: () => Promise<string | null>;
  /** Syncs a file; `done` gets the synced copy (it goes on even if the tracks card closes). */
  sync: (request: SyncRequest, done: (synced: Synced) => void) => Promise<void>;
  cancel: () => void;
}

/** Subtitle sync's optional download (docs/subtitle-sync.md), and the sync running now. */
export const useSubSync = create<SubSyncState>((set, get) => ({
  status: null,
  running: null,
  progress: null,
  failure: null,
  load: async () => set({ status: await ipc.subsyncStatus() }),
  apply: (status) => set({ status }),
  onProgress: (progress) => set({ progress }),
  install: async () => {
    // Failures arrive as a status of their own; this only waits for the end.
    await ipc.subsyncInstall().catch(() => {});
    set({ status: await ipc.subsyncStatus() });
  },
  remove: async () => {
    try {
      await ipc.subsyncRemove();
      set({ status: await ipc.subsyncStatus() });
      return null;
    } catch (error) {
      return toAppError(error).message;
    }
  },
  sync: async (request, done) => {
    if (get().running) return;
    set({ running: request.id, progress: null, failure: null });
    try {
      done(
        await ipc.syncSubtitles(
          request.file,
          request.subs,
          request.track,
          request.stream,
          request.language,
          request.charset,
        ),
      );
    } catch (error) {
      const failed = toAppError(error);
      if (failed.kind !== "cancelled") set({ failure: { key: request.id, message: failed.message } });
    } finally {
      set({ running: null, progress: null });
      // A new synced copy is on the disk: whether anything uses it is told even
      // when what is used hasn't changed (the film was closed, or another opened,
      // while it synced). It is in its draft, if it has one, by the time this looks.
      told = null;
      review();
    }
  },
  cancel: () => void ipc.syncCancel(),
}));

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
