import type { KeptTrack, MkvContents, MkvKind, TrackEdit } from "@/types";

/** A track as the user has it: kept or not, and what the copy says of it. */
export interface DraftTrack extends KeptTrack {
  kind: MkvKind;
  keep: boolean;
}

/** What the user has chosen of an MKV's tracks, beside the file as it is. */
export interface Draft {
  /** The file, version and languages it was made for: other ones start afresh. */
  key: string;
  version: string;
  languages: string;
  tracks: DraftTrack[];
  /** The tracks as the file has them: what "changed" is measured against. */
  before: DraftTrack[];
  chapters: boolean;
  attachments: boolean;
  /** The user's languages chose what it keeps (and nothing has undone that). */
  mine: boolean;
}

/**
 * What a file opens with: what the user's languages keep, if they choose
 * anything, or else everything, as the file has it. A language read from a
 * track's words is the one written.
 */
export function draftOf(contents: MkvContents, key: string, version: string, languages: string): Draft {
  const asItIs = asTheFileIs(contents, key, version, languages);
  return contents.suggestion ? mineOf(asItIs, contents.suggestion.edit) : asItIs;
}

/** What `edit` keeps of the file, as the user's languages chose it. */
export function mineOf(draft: Draft, edit: TrackEdit): Draft {
  const tracks = draft.before.map((track) => {
    const kept = edit.tracks.find((candidate) => candidate.index === track.index);
    return kept ? { ...track, ...kept, keep: true } : { ...track, keep: false };
  });
  return { ...draft, tracks, mine: true };
}

/** Everything, as the file has it. */
export function asItIs(draft: Draft): Draft {
  return { ...draft, tracks: draft.before, chapters: true, attachments: true, mine: false };
}

function asTheFileIs(contents: MkvContents, key: string, version: string, languages: string): Draft {
  const tracks = contents.tracks.map(
    (track): DraftTrack => ({
      index: track.index,
      kind: track.kind,
      keep: true,
      language: track.language,
      name: track.name ?? "",
      default: track.default,
      forced: track.forced,
      sdh: track.sdh,
      commentary: track.commentary,
      described: track.described,
      original: track.original,
    }),
  );
  return { key, version, languages, tracks, before: tracks, chapters: true, attachments: true, mine: false };
}

/** What the backend is asked to make. */
export function editOf(draft: Draft): TrackEdit {
  return {
    tracks: draft.tracks.filter((track) => track.keep).map(({ kind: _kind, keep: _keep, ...kept }) => kept),
    chapters: draft.chapters,
    attachments: draft.attachments,
  };
}

const FIELDS = ["index", "language", "name", "default", "forced", "sdh", "commentary", "described", "original"] as const;

/** How the draft differs from the file: tracks left out, and whether anything at all changed. */
export function changesOf(draft: Draft): { removed: number; kept: number; changed: boolean } {
  const kept = draft.tracks.filter((track) => track.keep);
  const removed = draft.tracks.length - kept.length;
  const moved = kept.some((track, place) => FIELDS.some((field) => track[field] !== draft.before[place]?.[field]));
  return { removed, kept: kept.length, changed: removed > 0 || moved || !draft.chapters || !draft.attachments };
}

/** About how large the copy is, where the file counts its tracks' bytes. */
export function sizeOf(draft: Draft, contents: MkvContents): number | null {
  let total = 0;
  for (const track of draft.tracks) {
    if (!track.keep) continue;
    const bytes = contents.tracks.find((candidate) => candidate.index === track.index)?.bytes;
    if (bytes == null) return null;
    total += bytes;
  }
  return total;
}

/** A change to one track; marking one to play first unmarks the others of its kind. */
export function withTrack(draft: Draft, index: number, change: Partial<DraftTrack>): Draft {
  const target = draft.tracks.find((track) => track.index === index);
  if (!target) return draft;
  return {
    ...draft,
    tracks: draft.tracks.map((track) => {
      if (track.index === index) return { ...track, ...change };
      if (change.default && track.kind === target.kind) return { ...track, default: false };
      return track;
    }),
  };
}

/** Moves a track one place up or down among those of its kind. */
export function moved(draft: Draft, index: number, by: -1 | 1): Draft {
  const from = draft.tracks.findIndex((track) => track.index === index);
  const kind = draft.tracks[from]?.kind;
  let to = from + by;
  while (to >= 0 && to < draft.tracks.length && draft.tracks[to]?.kind !== kind) to += by;
  if (from < 0 || to < 0 || to >= draft.tracks.length) return draft;
  const tracks = [...draft.tracks];
  const [track] = tracks.splice(from, 1);
  if (track) tracks.splice(to, 0, track);
  return { ...draft, tracks };
}
