import { create } from "zustand";

export type Panel = "downloads" | "settings";

/** The pages of Settings, one shown at a time. */
export type SettingsSection = "downloads" | "sites" | "languages" | "sync" | "appearance" | "updates";

interface UiState {
  /** The panel that is open, if any: Activity slides over the window, Settings fills it. */
  panel: Panel | null;
  /** The page of Settings shown, kept while Siphon runs so it opens where it was left. */
  settingsSection: SettingsSection;
  /** Opens a panel; Settings on a page of its own when given one (a link to "your languages in Settings"). */
  openPanel: (panel: Panel, section?: SettingsSection) => void;
  closePanel: () => void;
  /** Opens the panel, or closes it if it is the one open: the title bar's buttons. */
  togglePanel: (panel: Panel) => void;
  showSettingsSection: (section: SettingsSection) => void;
}

export const useUi = create<UiState>((set) => ({
  panel: null,
  settingsSection: "downloads",
  openPanel: (panel, section) => set(section ? { panel, settingsSection: section } : { panel }),
  closePanel: () => set({ panel: null }),
  togglePanel: (panel) => set((state) => ({ panel: state.panel === panel ? null : panel })),
  showSettingsSection: (settingsSection) => set({ settingsSection }),
}));
