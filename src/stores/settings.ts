import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc";
import type { Settings, SettingsView } from "@/types";

const SETTINGS_KEY = ["settings"];

/** The settings, shared by every part of the window that shows one. */
export function useSettings() {
  return useQuery({ queryKey: SETTINGS_KEY, queryFn: ipc.getSettings, staleTime: Number.POSITIVE_INFINITY });
}

/** Saves settings; everything showing them updates with what was stored. */
export function useSaveSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (next: Settings) => ipc.setSettings(next),
    onSuccess: (saved) => queryClient.setQueryData(SETTINGS_KEY, saved),
  });
}

/** The backend wants plain settings back, without the display-only extras. */
export function stripView({ default_output_dir: _default, ...settings }: SettingsView): Settings {
  return settings;
}
