import { primary } from "@/lib/languages";
import type {
  AddedFile,
  Chapter,
  KeptTrack,
  MkvContents,
  MkvKind,
  MkvTrack,
  PickedFile,
  SyncSummary,
  TrackEdit,
} from "@/types";

/**
 * A track as the user has it: kept or not, and what the copy says of it.
 * A track added from another file has an `index` of its own in the draft
 * (see `addTracks`), and its place in that file as `stream`.
 */
export interface DraftTrack extends KeptTrack {
  kind: MkvKind;
  keep: boolean;
  /** Added from another file: its place among that file's streams. */
  stream?: number;
}

/** A file tracks were added from, and where its tracks' draft indexes start. */
export interface DraftAdded extends AddedFile {
  base: number;
  /** Synced to the speech (docs/subtitle-sync.md): the file as it was given, to go back to, and what was found. */
  synced?: { original: AddedFile; summary: SyncSummary };
}

/**
 * The file added as `from` swapped for its copy synced to the speech, every
 * choice made of its tracks kept: their language, name, flags and place.
 */
export function withSynced(draft: Draft, from: string, synced: AddedFile, summary: SyncSummary): Draft {
  const old = draft.added.find((added) => added.key === from);
  if (!old) return draft;
  const { base, synced: before, ...given } = old;
  const original = before?.original ?? given;
  return {
    ...draft,
    added: draft.added.map((added) => (added.key === from ? { ...synced, base, synced: { original, summary } } : added)),
    tracks: draft.tracks.map((track) =>
      track.from === from ? { ...track, from: synced.key, charset: synced.charset } : track,
    ),
  };
}

/** Back to the file as it was given, before it was synced. */
export function withoutSync(draft: Draft, key: string): Draft {
  const added = draft.added.find((candidate) => candidate.key === key);
  if (!added?.synced) return draft;
  const { original } = added.synced;
  return {
    ...draft,
    added: draft.added.map((candidate) => (candidate.key === key ? { ...original, base: added.base } : candidate)),
    tracks: draft.tracks.map((track) =>
      track.from === key ? { ...track, from: original.key, charset: original.charset } : track,
    ),
  };
}

/** The languages subtitle sync can time words in: Qwen3-ForcedAligner's (`TIMED` in `src-tauri/sync/siphon_sync.py`). */
const TIMED = new Set(["en", "zh", "yue", "fr", "de", "it", "ja", "ko", "pt", "ru", "es"]);

/**
 * The sound a sync listens to, of the film's own: one in a language its words
 * can be timed in, else one with no language tag, else any (which the sync
 * refuses, saying why); never commentary while there is other sound. Among
 * those, the one that plays first, else the first kept, else the first. Kept
 * or not, every sound track runs on the film's clock: a Greek dub that plays
 * first doesn't stop the English beside it from being listened to.
 */
export function speechTrack(draft: Draft): DraftTrack | undefined {
  const own = draft.tracks.filter((track) => track.kind === "sound" && !track.from);
  const speech = own.filter((track) => !track.commentary);
  const pick = (tracks: DraftTrack[]) =>
    tracks.find((track) => track.keep && track.default) ?? tracks.find((track) => track.keep) ?? tracks[0];
  return (
    pick(speech.filter((track) => track.language && TIMED.has(primary(track.language)))) ??
    pick(speech.filter((track) => !track.language)) ??
    pick(speech) ??
    pick(own)
  );
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
  /** Files tracks were added from (docs/mkv-editor.md, step 6). */
  added: DraftAdded[];
  /** The chapters the copy gets, as chosen; null: as the file has them (or none, `chapters` false). */
  chapterList: Chapter[] | null;
  /** Where a chapter list taken from a file came from: "Film.chapters.xml". */
  chaptersFrom: string | null;
  /** The file's attached files left out, by their place among its streams. */
  leftOutAttachments: number[];
  /** Fonts to attach. */
  fonts: PickedFile[];
  /** The kind of file it is. */
  container: MkvContents["container"];
  /** An MP4 saved as an MKV: the copy holds what an MP4 can't. */
  mkv: boolean;
}

/** What an MP4 holds without converting it (`local/mkv/edit.rs`, `fits_mp4`). */
export function fitsMp4(track: MkvTrack): boolean {
  switch (track.kind) {
    case "video":
      return ["h264", "hevc", "av1", "vp9", "mpeg4", "mpeg2video", "mjpeg", "prores"].includes(track.codec);
    case "sound":
      return ["aac", "mp3", "ac3", "eac3", "alac", "flac", "opus"].includes(track.codec);
    case "subtitles":
      return ["subrip", "ass", "ssa", "webvtt", "text", "mov_text"].includes(track.codec);
  }
}

/** The copy is an MKV: an MKV's and an MPEG-TS's are, and an MP4's when asked. */
export function savedAsMkv(draft: Draft): boolean {
  return draft.container !== "mp4" || draft.mkv;
}

/** Each file added from gets indexes of its own in a draft: far past any file's streams. */
const ADDED_BASE = 100_000;

/**
 * What a file opens with: what the user's languages keep, if they choose
 * anything, or else everything, as the file has it. A language read from a
 * track's words is the one written.
 */
export function draftOf(contents: MkvContents, key: string, version: string, languages: string): Draft {
  const asItIs = asTheFileIs(contents, key, version, languages);
  return contents.suggestion ? mineOf(asItIs, contents.suggestion.edit) : asItIs;
}

/** The tracks added from other files, in their places among `tracks`. */
function withAdded(draft: Draft, tracks: DraftTrack[]): DraftTrack[] {
  return draft.tracks.filter((track) => track.from).reduce(placed, tracks);
}

/** `tracks` with `track` after the last of its kind (or last of all). */
function placed(tracks: DraftTrack[], track: DraftTrack): DraftTrack[] {
  const last = tracks.findLastIndex((candidate) => candidate.kind === track.kind);
  const at = last >= 0 ? last + 1 : tracks.length;
  return [...tracks.slice(0, at), track, ...tracks.slice(at)];
}

/** What `edit` keeps of the file, as the user's languages chose it. */
export function mineOf(draft: Draft, edit: TrackEdit): Draft {
  const tracks = draft.before.map((track) => {
    const kept = edit.tracks.find((candidate) => candidate.index === track.index && !candidate.from);
    return kept ? { ...track, ...kept, keep: true } : { ...track, keep: false };
  });
  return { ...draft, tracks: withAdded(draft, tracks), mine: true };
}

/** Everything, as the file has it, and whatever was added. */
export function asItIs(draft: Draft): Draft {
  return {
    ...draft,
    tracks: withAdded(draft, draft.before),
    chapters: true,
    attachments: true,
    chapterList: null,
    chaptersFrom: null,
    leftOutAttachments: [],
    mine: false,
  };
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
  return {
    key,
    version,
    languages,
    tracks,
    before: tracks,
    chapters: true,
    attachments: true,
    mine: false,
    added: [],
    chapterList: null,
    chaptersFrom: null,
    leftOutAttachments: [],
    fonts: [],
    container: contents.container,
    mkv: false,
  };
}

/** The chapters the copy gets: as chosen, else the file's own, or none. */
export function chaptersOf(draft: Draft, contents: MkvContents): Chapter[] {
  if (draft.chapterList) return draft.chapterList;
  return draft.chapters ? contents.chapters.list : [];
}

/** The copy gets these chapters. */
export function withChapters(draft: Draft, chapters: Chapter[], from: string | null = draft.chaptersFrom): Draft {
  return { ...draft, chapters: true, chapterList: chapters, chaptersFrom: from };
}

/** Fonts to attach, each once. */
export function withFonts(draft: Draft, fonts: PickedFile[]): Draft {
  const more = fonts.filter((font) => !draft.fonts.some((known) => known.key === font.key));
  // An MP4 can't hold fonts: its copy becomes an MKV.
  return { ...draft, fonts: [...draft.fonts, ...more], mkv: draft.mkv || draft.container === "mp4" };
}

/**
 * Adds a file's tracks to the draft, each after the last of its kind: its
 * sound and subtitles kept, a video's picture not (a second picture is
 * rarely what is wanted). Subtitles in the user's first language play first
 * when no sound is in it and no subtitles play first yet.
 */
export function addTracks(draft: Draft, file: AddedFile): Draft {
  const without = removeAdded(draft, file.key);
  const base = ADDED_BASE * (Math.max(0, ...without.added.map((added) => added.base / ADDED_BASE)) + 1);
  const first = draft.languages.split(",")[0] ?? "";
  const soundInFirst = without.tracks.some(
    (track) => track.keep && track.kind === "sound" && track.language && primary(track.language) === primary(first),
  );
  let subtitlesFirst = without.tracks.some((track) => track.keep && track.kind === "subtitles" && track.default);
  let tracks = without.tracks;
  for (const track of file.contents.tracks) {
    const plays =
      track.kind === "subtitles" &&
      !subtitlesFirst &&
      !soundInFirst &&
      !track.forced &&
      track.language !== null &&
      first !== "" &&
      primary(track.language) === primary(first);
    if (plays) subtitlesFirst = true;
    tracks = placed(tracks, {
      index: base + track.index,
      stream: track.index,
      from: file.key,
      charset: file.charset,
      kind: track.kind,
      keep: track.kind !== "video",
      language: track.language,
      name: track.name ?? "",
      default: plays,
      forced: track.forced,
      sdh: track.sdh,
      commentary: track.commentary,
      described: track.described,
      original: false,
    });
  }
  // What an MP4 can't hold makes its copy an MKV.
  const misfit = file.contents.tracks.some((track) => track.kind !== "video" && !fitsMp4(track));
  return {
    ...without,
    tracks,
    added: [...without.added, { ...file, base }],
    mkv: without.mkv || (without.container === "mp4" && misfit),
  };
}

/** The draft without the tracks added from `key`. */
export function removeAdded(draft: Draft, key: string): Draft {
  return {
    ...draft,
    tracks: draft.tracks.filter((track) => track.from !== key),
    added: draft.added.filter((added) => added.key !== key),
  };
}

/** What reading found of a draft's track: in the MKV, or in the file it was added from. */
export function readOf(draft: Draft, contents: MkvContents, track: DraftTrack): MkvTrack | undefined {
  if (!track.from) return contents.tracks.find((candidate) => candidate.index === track.index);
  return addedOf(draft, track)?.contents.tracks.find((candidate) => candidate.index === track.stream);
}

/** The file a track was added from. */
export function addedOf(draft: Draft, track: DraftTrack): DraftAdded | undefined {
  return track.from ? draft.added.find((added) => added.key === track.from) : undefined;
}

/** What the backend is asked to make. */
export function editOf(draft: Draft): TrackEdit {
  return {
    tracks: draft.tracks
      .filter((track) => track.keep)
      .map(({ kind: _kind, keep: _keep, stream, ...kept }) => ({ ...kept, index: stream ?? kept.index })),
    chapters: draft.chapters,
    attachments: draft.attachments,
    chapter_list: draft.chapterList,
    left_out_attachments: draft.leftOutAttachments,
    fonts: draft.fonts.map((font) => font.key),
    mkv: draft.mkv,
  };
}

const FIELDS = ["index", "language", "name", "default", "forced", "sdh", "commentary", "described", "original"] as const;

/**
 * How the draft differs from the file: its own tracks kept and left out,
 * tracks added, and whether anything at all changed.
 */
export function changesOf(draft: Draft): { removed: number; kept: number; added: number; changed: boolean } {
  const kept = draft.tracks.filter((track) => track.keep);
  const own = kept.filter((track) => !track.from);
  const added = kept.length - own.length;
  const removed = draft.before.length - own.length;
  const moved = kept.some((track, place) => FIELDS.some((field) => track[field] !== draft.before[place]?.[field]));
  return {
    removed,
    kept: own.length,
    added,
    changed:
      removed > 0 ||
      added > 0 ||
      moved ||
      !draft.chapters ||
      !draft.attachments ||
      draft.chapterList !== null ||
      draft.leftOutAttachments.length > 0 ||
      draft.fonts.length > 0 ||
      draft.mkv,
  };
}

/** About how large the copy is, where the file counts its tracks' bytes. */
export function sizeOf(draft: Draft, contents: MkvContents): number | null {
  let total = 0;
  for (const track of draft.tracks) {
    if (!track.keep) continue;
    const from = addedOf(draft, track);
    if (from) {
      const bytes = readOf(draft, contents, track)?.bytes;
      total += bytes ?? from.size / Math.max(1, from.contents.tracks.length);
      continue;
    }
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
