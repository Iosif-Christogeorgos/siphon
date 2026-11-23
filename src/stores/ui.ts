import { create } from "zustand";

export type Panel = "downloads" | "settings";

interface UiState {
  /** The slide-over panel that is open, if any. */
  panel: Panel | null;
  openPanel: (panel: Panel) => void;
  closePanel: () => void;
  /** Opens the panel, or closes it if it is the one open: the title bar's buttons. */
  togglePanel: (panel: Panel) => void;
}

export const useUi = create<UiState>((set) => ({
  panel: null,
  openPanel: (panel) => set({ panel }),
  closePanel: () => set({ panel: null }),
  togglePanel: (panel) => set((state) => ({ panel: state.panel === panel ? null : panel })),
}));
