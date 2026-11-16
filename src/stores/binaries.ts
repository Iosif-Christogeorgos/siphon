import { create } from "zustand";
import { ipc } from "@/lib/ipc";
import type { Tool, ToolStatus } from "@/types";

interface BinariesState {
  /** `null` until the first status arrives from the backend. */
  tools: Record<Tool, ToolStatus> | null;
  syncing: boolean;
  apply: (status: ToolStatus) => void;
  /** Loads the current state, then installs/updates in the background. */
  sync: () => Promise<void>;
}

const byTool = (statuses: ToolStatus[]) =>
  Object.fromEntries(statuses.map((status) => [status.tool, status])) as Record<Tool, ToolStatus>;

export const useBinaries = create<BinariesState>((set, get) => ({
  tools: null,
  syncing: false,

  apply: (status) => {
    const tools = get().tools;
    if (tools) set({ tools: { ...tools, [status.tool]: status } });
  },

  sync: async () => {
    if (get().syncing) return;
    set({ syncing: true });
    try {
      set({ tools: byTool(await ipc.binariesStatus()) });
      set({ tools: byTool(await ipc.binariesSync()) });
    } finally {
      set({ syncing: false });
    }
  },
}));

/**
 * A tool is usable when some version is installed, even while a newer one
 * downloads. (`ready` without a version is a system-provided tool.)
 */
export const isUsable = (status: ToolStatus) => status.version !== null || status.state === "ready";

export const selectAllUsable = (state: BinariesState) =>
  state.tools !== null && Object.values(state.tools).every(isUsable);
