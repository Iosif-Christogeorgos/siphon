import type { AddedFile, Chapter, PickedFile } from "@/types";
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

/** What a file picked to add turned out to hold, read. */
export type PickedRead =
  | { kind: "tracks"; file: PickedFile; added: AddedFile }
  | { kind: "chapters"; file: PickedFile; chapters: Chapter[] }
  | { kind: "font"; file: PickedFile };

/** Fonts are attached as they are; chapters files read for their chapters; the rest for their tracks. */
export function kindOfPicked(name: string): PickedRead["kind"] {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (["ttf", "otf", "ttc"].includes(ext)) return "font";
  if (["xml", "txt"].includes(ext)) return "chapters";
  return "tracks";
}

/** Reads a file picked to add, by what it is. */
export async function readPicked(file: PickedFile): Promise<PickedRead> {
  switch (kindOfPicked(file.name)) {
    case "font":
      return { kind: "font", file };
    case "chapters":
      return { kind: "chapters", file, chapters: await ipc.readChapters(file.key) };
    case "tracks":
      return { kind: "tracks", file, added: await ipc.readAdded(file.key) };
  }
}
