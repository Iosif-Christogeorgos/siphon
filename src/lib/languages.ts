import type { SubtitleTrack } from "@/types";

const names = (() => {
  try {
    return new Intl.DisplayNames(["en"], { type: "language" });
  } catch {
    return null;
  }
})();

/** A language tag's name in English: "el" → "Greek", "pt-br" → "Brazilian Portuguese"; the tag if unknown. */
export function languageName(tag: string): string {
  try {
    return names?.of(tag) ?? tag;
  } catch {
    return tag;
  }
}

/** The part of a tag that names the language alone: "pt-br" → "pt". */
export const primary = (tag: string) => tag.split("-")[0] ?? tag;

/** The language Windows is set to, without its region: "el" for Greek (Greece). */
export function systemLanguage(): string {
  return primary((navigator.language || "en").toLowerCase());
}

/** A language subtitles can be chosen in. */
export interface SubtitleLanguage {
  /** The tag remembered when it's chosen: "en", "pt-br". */
  lang: string;
  name: string;
  /** Its track comes from speech recognition. */
  automatic: boolean;
}

const KIND_ORDER = { uploaded: 0, automatic: 1 } as const;
const byBytes = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

/**
 * The track Siphon uses for `wanted`, exactly as `pick` in `plan/subtitles.rs`
 * chooses it: people's over speech recognition, then the exact region, then
 * the plain language. Undefined when the video has none in that language.
 */
export function pickTrack(tracks: SubtitleTrack[], wanted: string): SubtitleTrack | undefined {
  const tag = wanted.toLowerCase();
  return tracks
    .filter((track) => primary(track.lang) === primary(tag))
    .sort(
      (a, b) =>
        KIND_ORDER[a.kind] - KIND_ORDER[b.kind] ||
        Number(a.lang !== tag) - Number(b.lang !== tag) ||
        a.lang.length - b.lang.length ||
        byBytes(a.key, b.key),
    )[0];
}

/**
 * One choice per language the video's subtitles come in: each people's track,
 * and speech recognition only where no people's track has its language (it
 * would never be used there). The system language first, then by name.
 */
export function subtitleLanguages(tracks: SubtitleTrack[]): SubtitleLanguage[] {
  const people = new Set(tracks.filter((track) => track.kind === "uploaded").map((track) => primary(track.lang)));
  const seen = new Map<string, SubtitleLanguage>();
  for (const track of tracks) {
    if (seen.has(track.lang)) continue;
    if (track.kind === "automatic" && people.has(primary(track.lang))) continue;
    seen.set(track.lang, {
      lang: track.lang,
      name: track.name ?? languageName(track.lang),
      automatic: track.kind === "automatic",
    });
  }
  const mine = systemLanguage();
  return [...seen.values()].sort(
    (a, b) => Number(primary(b.lang) === mine) - Number(primary(a.lang) === mine) || a.name.localeCompare(b.name),
  );
}

/**
 * For a playlist, whose videos each have their own: the language chosen
 * before, the system language and English. Each video uses it if it has it.
 */
export function playlistLanguages(wanted: string | null): SubtitleLanguage[] {
  const tags = [wanted, systemLanguage(), "en"].filter((tag): tag is string => Boolean(tag));
  return [...new Set(tags)].map((lang) => ({ lang, name: languageName(lang), automatic: false }));
}
