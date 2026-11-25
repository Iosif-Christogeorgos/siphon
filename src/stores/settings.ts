import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc";
import type { Settings, SettingsView } from "@/types";

const SETTINGS_KEY = ["settings"];

/** The settings, shared by every part of the window that shows one. */
export function useSettings() {
  return useQuery({ queryKey: SETTINGS_KEY, queryFn: ipc.getSettings, staleTime: Number.POSITIVE_INFINITY });
}

/**
 * Saves settings; everything showing them updates with what was stored.
 * A choice shows at once, before the backend has answered, and goes back if
 * it couldn't be stored.
 */
export function useSaveSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (next: Settings) => ipc.setSettings(next),
    onMutate: async (next) => {
      const turn = ++saves;
      const before = queryClient.getQueryData<SettingsView>(SETTINGS_KEY);
      if (!before) return { turn, before };
      await queryClient.cancelQueries({ queryKey: SETTINGS_KEY });
      queryClient.setQueryData<SettingsView>(SETTINGS_KEY, { ...before, ...next });
      return { turn, before };
    },
    // Only the latest save's answer is shown: an earlier one arriving after a
    // newer choice would flick the control back for a moment.
    onError: (_error, _next, context) => {
      if (context?.before && context.turn === saves) queryClient.setQueryData(SETTINGS_KEY, context.before);
    },
    onSuccess: (saved, _next, context) => {
      if (context?.turn === saves) queryClient.setQueryData(SETTINGS_KEY, saved);
    },
  });
}

/** Counts saves, so an answer can tell whether a newer save has been made since. */
let saves = 0;

/** The backend wants plain settings back, without the display-only extras. */
export function stripView({
  default_output_dir: _folder,
  default_languages: _languages,
  ...settings
}: SettingsView): Settings {
  return settings;
}

/** The user's languages, best first: those chosen, or those taken as theirs until they choose. */
export function myLanguages(settings: SettingsView): string[] {
  return settings.languages.length > 0 ? settings.languages : settings.default_languages;
}
