import { getVersion } from "@tauri-apps/api/app";
import { create } from "zustand";
import { ipc, toAppError } from "@/lib/ipc";
import { useToasts } from "@/stores/toasts";

interface AppUpdateState {
  /** The running version of Siphon. */
  current: string | null;
  /** A newer published version, if one was found. */
  available: string | null;
  installing: boolean;
  /** Looks for a newer release. Failures are silent: being offline is not news. */
  check: () => Promise<void>;
  /** Downloads the update, installs it and restarts the app. */
  install: () => Promise<void>;
}

export const useAppUpdate = create<AppUpdateState>((set, get) => ({
  current: null,
  available: null,
  installing: false,

  check: async () => {
    if (get().current === null) set({ current: await getVersion().catch(() => null) });
    const available = await ipc.appUpdateCheck().catch(() => null);
    set({ available });
  },

  install: async () => {
    if (get().installing) return;
    set({ installing: true });
    useToasts.getState().show({ id: "app-update", title: "Updating Siphon…", description: "It will restart in a moment." });
    try {
      await ipc.appUpdateInstall();
    } catch (error) {
      // For example: downloads are still running.
      useToasts.getState().show({ id: "app-update", title: "Not updated yet", description: toAppError(error).message });
    } finally {
      set({ installing: false });
    }
  },
}));
