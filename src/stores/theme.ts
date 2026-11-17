import { create } from "zustand";
import { persist } from "zustand/middleware";

/** "system" follows the computer's light or dark mode. */
export type Theme = "system" | "light" | "dark";

interface ThemeState {
  theme: Theme;
  setTheme: (theme: Theme) => void;
}

/**
 * The colour theme. Kept in the webview's storage rather than settings.json,
 * because only the interface uses it.
 */
export const useTheme = create<ThemeState>()(
  persist(
    (set) => ({
      theme: "system",
      setTheme: (theme) => set({ theme }),
    }),
    { name: "siphon.theme", partialize: ({ theme }) => ({ theme }) },
  ),
);

/** Marks <html> with the chosen theme; the colours in index.css follow it. */
function apply(theme: Theme) {
  const root = document.documentElement;
  if (theme === "light" || theme === "dark") root.dataset.theme = theme;
  else delete root.dataset.theme;
}

// Applied as soon as this module loads, which is before the first frame
// (see main.tsx), so the window never shows in the wrong theme.
apply(useTheme.getState().theme);
useTheme.subscribe((state) => apply(state.theme));
