import { create } from "zustand";
import { ipc, toAppError } from "@/lib/ipc";
import type { SubSyncStatus, Synced, SyncProgress } from "@/types";

interface SyncRequest {
  /** The film's key. */
  file: string;
  /** The added subtitle file's key. */
  subs: string;
  /** The film's sound stream to listen to, and its language tag. */
  stream: number;
  language: string | null;
  charset: string | null;
}

interface SubSyncState {
  /** Null until the backend has said. */
  status: SubSyncStatus | null;
  /** The subtitle file being synced now, by its key, and how far it has got. */
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
    set({ running: request.subs, progress: null, failure: null });
    try {
      done(await ipc.syncSubtitles(request.file, request.subs, request.stream, request.language, request.charset));
    } catch (error) {
      const failed = toAppError(error);
      if (failed.kind !== "cancelled") set({ failure: { key: request.subs, message: failed.message } });
    } finally {
      set({ running: null, progress: null });
    }
  },
  cancel: () => void ipc.syncCancel(),
}));
