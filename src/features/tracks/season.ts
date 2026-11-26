import { primary } from "@/lib/languages";
import type { MkvContents, MkvTrack } from "@/types";
import {
  addedOf,
  asItIs,
  canSync,
  changesOf,
  type Draft,
  type DraftTrack,
  isSynced,
  mineOf,
  readOf,
  sizeOf,
  syncIdOf,
} from "./draft";
import type { SyncTarget } from "./SyncToSpeech";

/**
 * A season: several MKVs whose tracks are chosen together (docs/mkv-editor.md).
 * Each file keeps its own draft, made by the user's languages as a single
 * file's is; a choice made for the season (keep English commentary, Greek
 * subtitles play first) is made in every file that has such a track. A track
 * whose language isn't certain is never changed by a season's choice: it is
 * kept, and its file marked, to be seen to by hand.
 */
export interface SeasonFile {
  key: string;
  /** "Show S01E03.mkv": what files to add are matched by. */
  name: string;
  contents: MkvContents;
  draft: Draft;
}

/** What a sound or subtitle track is for, beside its language. */
export type Role = "main" | "commentary" | "described" | "forced" | "sdh";

/** Tracks of one kind, language and role, across the season: "English commentary". */
export interface SeasonRow {
  key: string;
  kind: "sound" | "subtitles";
  language: string;
  role: Role;
  /** Added from other files, rather than the episodes' own. */
  added: boolean;
  /** How many files have one. */
  files: number;
  /** Whether the copies keep them: in every file that has one, in none, or in some. */
  keep: "all" | "none" | "some";
  /** Whether they play first, the same way. */
  first: "all" | "none" | "some";
}

function roleOf(track: MkvTrack): Role {
  if (track.commentary) return "commentary";
  if (track.described) return "described";
  if (track.kind === "subtitles" && track.forced) return "forced";
  if (track.kind === "subtitles" && track.sdh) return "sdh";
  return "main";
}

/**
 * The row a draft's track belongs to; none for video, or a track whose
 * language isn't certain. Tracks added from other files have rows of their own.
 */
function rowKey(file: SeasonFile, track: DraftTrack): string | null {
  const read = readOf(file.draft, file.contents, track);
  if (!read || read.kind === "video" || !read.certain || !read.language) return null;
  return `${read.kind}:${primary(read.language)}:${roleOf(read)}${track.from ? ":added" : ""}`;
}

const ROLES: Role[] = ["main", "forced", "sdh", "commentary", "described"];

/** The season's rows: sound before subtitles, the first language met first, the main tracks before the rest. */
export function rowsOf(files: SeasonFile[]): SeasonRow[] {
  type Counting = SeasonRow & { kept: number; firsts: number; members: number; seen: number; inFiles: Set<string> };
  const rows = new Map<string, Counting>();
  let seen = 0;
  for (const file of files) {
    for (const track of file.draft.tracks) {
      const key = rowKey(file, track);
      const read = readOf(file.draft, file.contents, track);
      if (!key || !read?.language || read.kind === "video") continue;
      let row = rows.get(key);
      if (!row) {
        row = {
          key,
          kind: read.kind,
          language: primary(read.language),
          role: roleOf(read),
          added: Boolean(track.from),
          files: 0,
          keep: "all",
          first: "none",
          kept: 0,
          firsts: 0,
          members: 0,
          seen: seen++,
          inFiles: new Set(),
        };
        rows.set(key, row);
      }
      row.members += 1;
      row.inFiles.add(file.key);
      if (track.keep) row.kept += 1;
      if (track.keep && track.default) row.firsts += 1;
    }
  }
  const state = (count: number, of: number) => (count === 0 ? "none" : count === of ? "all" : "some");
  const firstLanguage = new Map<string, number>();
  for (const row of rows.values()) {
    if (!firstLanguage.has(row.language)) firstLanguage.set(row.language, row.seen);
  }
  return [...rows.values()]
    .map(
      ({ kept, firsts, members, seen: _seen, inFiles, ...row }): SeasonRow => ({
        ...row,
        files: inFiles.size,
        keep: state(kept, members),
        first: state(firsts, inFiles.size),
      }),
    )
    .sort(
      (a, b) =>
        Number(a.kind === "subtitles") - Number(b.kind === "subtitles") ||
        Number(a.added) - Number(b.added) ||
        (firstLanguage.get(a.language) ?? 0) - (firstLanguage.get(b.language) ?? 0) ||
        ROLES.indexOf(a.role) - ROLES.indexOf(b.role),
    );
}

/** Keeps, or leaves out, the row's tracks in every file. */
export function keepRow(files: SeasonFile[], key: string, keep: boolean): Map<string, Draft> {
  const drafts = new Map<string, Draft>();
  for (const file of files) {
    if (!file.draft.tracks.some((track) => rowKey(file, track) === key)) continue;
    drafts.set(file.key, {
      ...file.draft,
      tracks: file.draft.tracks.map((track) =>
        rowKey(file, track) === key ? { ...track, keep, default: keep && track.default } : track,
      ),
    });
  }
  return drafts;
}

/**
 * Makes the row's track play first in every file that has one, and the
 * others of its kind not; or, `on` false, none of the row's.
 */
export function firstRow(files: SeasonFile[], key: string, on: boolean): Map<string, Draft> {
  const drafts = new Map<string, Draft>();
  for (const file of files) {
    const first = file.draft.tracks.find((track) => rowKey(file, track) === key);
    if (!first) continue;
    drafts.set(file.key, {
      ...file.draft,
      tracks: file.draft.tracks.map((track) => {
        if (on && track.index === first.index) return { ...track, keep: true, default: true };
        if (on && track.kind === first.kind) return { ...track, default: false };
        if (!on && rowKey(file, track) === key) return { ...track, default: false };
        return track;
      }),
    });
  }
  return drafts;
}

/** A track of a row, in one file, that can be synced to the speech. */
export interface RowSync {
  file: SeasonFile;
  /** What names its sync while it waits or runs (`syncIdOf`). */
  id: string;
  target: SyncTarget;
  synced: boolean;
}

/**
 * The row's subtitles that can be synced to the speech, file by file, in the
 * season's order: those the copies keep, in text (see `canSync`).
 */
export function rowSyncs(files: SeasonFile[], key: string): RowSync[] {
  return files.flatMap((file) =>
    file.draft.tracks.flatMap((track): RowSync[] => {
      if (!track.keep || rowKey(file, track) !== key || !canSync(file.draft, file.contents, track)) return [];
      const added = addedOf(file.draft, track);
      return [
        {
          file,
          id: syncIdOf(file.key, file.draft, track),
          target: added ? { added } : { track: track.index },
          synced: isSynced(file.draft, track),
        },
      ];
    }),
  );
}

/** Every file as it is, or as the user's languages have it. */
export function allAsItIs(files: SeasonFile[]): Map<string, Draft> {
  return new Map(files.map((file) => [file.key, asItIs(file.draft)]));
}

export function allMine(files: SeasonFile[]): Map<string, Draft> {
  return new Map(
    files.map((file) => [
      file.key,
      file.contents.suggestion ? mineOf(file.draft, file.contents.suggestion.edit) : file.draft,
    ]),
  );
}

/**
 * What in a file isn't certain: tracks whose language isn't known or is in
 * doubt (its own, and those added to it), and doubts on the file.
 */
export function toCheck(contents: MkvContents, draft?: Draft): string[] {
  const unsure = (track: MkvTrack) =>
    track.kind === "video" || track.certain
      ? []
      : track.doubts.length > 0
        ? track.doubts
        : [`A ${track.kind === "sound" ? "sound" : "subtitle"} track’s language isn’t known.`];
  const added = (draft?.tracks ?? []).flatMap((track) => {
    const read = track.from && track.keep && draft ? readOf(draft, contents, track) : undefined;
    return read ? unsure(read) : [];
  });
  return [...contents.doubts, ...contents.tracks.flatMap(unsure), ...added];
}

/** What saving the season makes: how many copies, the tracks they leave out and add, and about how large they are. */
export function seasonSummary(files: SeasonFile[]): {
  copies: number;
  removed: number;
  added: number;
  size: number | null;
} {
  let copies = 0;
  let removed = 0;
  let added = 0;
  let size: number | null = 0;
  for (const file of files) {
    const changes = changesOf(file.draft);
    if (!changes.changed) continue;
    copies += 1;
    removed += changes.removed;
    added += changes.added;
    const bytes = sizeOf(file.draft, file.contents);
    size = size === null || bytes === null ? null : size + bytes;
  }
  return { copies, removed, added, size };
}
