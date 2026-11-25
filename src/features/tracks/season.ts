import { primary } from "@/lib/languages";
import type { MkvContents, MkvTrack } from "@/types";
import { asItIs, changesOf, type Draft, mineOf, sizeOf } from "./draft";

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

/** The row a track belongs to; none for video, or a track whose language isn't certain. */
function rowKey(track: MkvTrack): string | null {
  if (track.kind === "video" || !track.certain || !track.language) return null;
  return `${track.kind}:${primary(track.language)}:${roleOf(track)}`;
}

const ROLES: Role[] = ["main", "forced", "sdh", "commentary", "described"];

/** The season's rows: sound before subtitles, the first language met first, the main tracks before the rest. */
export function rowsOf(files: SeasonFile[]): SeasonRow[] {
  const rows = new Map<string, SeasonRow & { kept: number; firsts: number; members: number; seen: number }>();
  let seen = 0;
  for (const file of files) {
    for (const read of file.contents.tracks) {
      const key = rowKey(read);
      const track = file.draft.tracks.find((candidate) => candidate.index === read.index);
      if (!key || !track || !read.language || read.kind === "video") continue;
      let row = rows.get(key);
      if (!row) {
        row = {
          key,
          kind: read.kind,
          language: primary(read.language),
          role: roleOf(read),
          files: 0,
          keep: "all",
          first: "none",
          kept: 0,
          firsts: 0,
          members: 0,
          seen: seen++,
        };
        rows.set(key, row);
      }
      row.members += 1;
      if (track.keep) row.kept += 1;
      if (track.keep && track.default) row.firsts += 1;
    }
    for (const row of rows.values()) {
      if (file.contents.tracks.some((track) => rowKey(track) === row.key)) row.files += 1;
    }
  }
  const state = (count: number, of: number) => (count === 0 ? "none" : count === of ? "all" : "some");
  const firstLanguage = new Map<string, number>();
  for (const row of rows.values()) {
    if (!firstLanguage.has(row.language)) firstLanguage.set(row.language, row.seen);
  }
  return [...rows.values()]
    .map((row) => ({ ...row, keep: state(row.kept, row.members), first: state(row.firsts, row.files) }) as const)
    .sort(
      (a, b) =>
        Number(a.kind === "subtitles") - Number(b.kind === "subtitles") ||
        (firstLanguage.get(a.language) ?? 0) - (firstLanguage.get(b.language) ?? 0) ||
        ROLES.indexOf(a.role) - ROLES.indexOf(b.role),
    )
    .map(({ kept: _k, firsts: _f, members: _m, seen: _s, ...row }) => row);
}

/** Keeps, or leaves out, the row's tracks in every file. */
export function keepRow(files: SeasonFile[], key: string, keep: boolean): Map<string, Draft> {
  const drafts = new Map<string, Draft>();
  for (const file of files) {
    const members = new Set(file.contents.tracks.filter((track) => rowKey(track) === key).map((track) => track.index));
    if (members.size === 0) continue;
    drafts.set(file.key, {
      ...file.draft,
      tracks: file.draft.tracks.map((track) =>
        members.has(track.index) ? { ...track, keep, default: keep && track.default } : track,
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
    const members = file.contents.tracks.filter((track) => rowKey(track) === key);
    const first = members[0];
    if (!first) continue;
    const kind = first.kind;
    const memberSet = new Set(members.map((track) => track.index));
    drafts.set(file.key, {
      ...file.draft,
      tracks: file.draft.tracks.map((track) => {
        if (on && track.index === first.index) return { ...track, keep: true, default: true };
        if (on && track.kind === kind) return { ...track, default: false };
        if (!on && memberSet.has(track.index)) return { ...track, default: false };
        return track;
      }),
    });
  }
  return drafts;
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

/** What in a file isn't certain: tracks whose language isn't known or is in doubt, and doubts on the file. */
export function toCheck(contents: MkvContents): string[] {
  return [
    ...contents.doubts,
    ...contents.tracks.flatMap((track) =>
      track.kind === "video" || track.certain
        ? []
        : track.doubts.length > 0
          ? track.doubts
          : [`A ${track.kind === "sound" ? "sound" : "subtitle"} track’s language isn’t known.`],
    ),
  ];
}

/** What saving the season makes: how many copies, the tracks they leave out, and about how large they are. */
export function seasonSummary(files: SeasonFile[]): { copies: number; removed: number; size: number | null } {
  let copies = 0;
  let removed = 0;
  let size: number | null = 0;
  for (const file of files) {
    const changes = changesOf(file.draft);
    if (!changes.changed) continue;
    copies += 1;
    removed += changes.removed;
    const bytes = sizeOf(file.draft, file.contents);
    size = size === null || bytes === null ? null : size + bytes;
  }
  return { copies, removed, size };
}
