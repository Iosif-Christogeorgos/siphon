import { ipc } from "@/lib/ipc";
import { myLanguages, useSettings } from "@/stores/settings";

/**
 * The user's languages in a word ("el,en"), or null until settings are
 * loaded: a file's tracks are read again when they change, as what they
 * keep changes with them.
 */
export function useLanguagesKey(): string | null {
  const settings = useSettings();
  return settings.data ? myLanguages(settings.data).join(",") : null;
}

/** Reading an MKV's tracks: the same query for one file and for each of a season's. */
export function tracksQuery(fileKey: string, version: string, languages: string | null, enabled = true) {
  return {
    queryKey: ["tracks", fileKey, version, languages],
    queryFn: () => ipc.readTracks(fileKey),
    enabled: enabled && languages !== null,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  };
}
