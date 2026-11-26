import { useQuery } from "@tanstack/react-query";
import {
  ArrowDown,
  ArrowUp,
  Captions,
  Check,
  ChevronDown,
  File as FileIcon,
  Film,
  Image,
  Plus,
  Tag,
  TriangleAlert,
  Type,
  Volume2,
  X,
  Zap,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Hint } from "@/components/ui/tooltip";
import { Note } from "@/features/options/PlanCard";
import { formatBytes, formatDuration } from "@/lib/format";
import { ipc, toAppError } from "@/lib/ipc";
import { useUi } from "@/stores/ui";
import { COMMON_LANGUAGES, languageName, primary, systemLanguage } from "@/lib/languages";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { Attachment, MkvContents, MkvKind, MkvTrack, Suggestion } from "@/types";
import {
  addedOf,
  addTracks,
  asItIs,
  chaptersOf,
  changesOf,
  type Draft,
  type DraftAdded,
  type DraftTrack,
  draftOf,
  fitsMp4,
  mineOf,
  moved,
  readOf,
  savedAsMkv,
  sizeOf,
  withChapters,
  withFonts,
  withTrack,
} from "./draft";
import { FlagToggle } from "./FlagToggle";
import { type PickedRead, readPicked, tracksQuery, useLanguagesKey } from "./reading";

const KINDS: { kind: MkvKind; label: string; icon: typeof Film }[] = [
  { kind: "video", label: "Video", icon: Film },
  { kind: "sound", label: "Sound", icon: Volume2 },
  { kind: "subtitles", label: "Subtitles", icon: Captions },
];

/** ffprobe's codec names, as people know them. */
const CODECS: Record<string, string> = {
  h264: "H.264",
  hevc: "HEVC",
  av1: "AV1",
  vp9: "VP9",
  mpeg2video: "MPEG-2",
  aac: "AAC",
  ac3: "Dolby Digital",
  eac3: "Dolby Digital Plus",
  truehd: "Dolby TrueHD",
  dts: "DTS",
  flac: "FLAC",
  opus: "Opus",
  mp3: "MP3",
  vorbis: "Vorbis",
  subrip: "SRT",
  ass: "ASS",
  ssa: "SSA",
  webvtt: "WebVTT",
  mov_text: "Timed text",
  hdmv_pgs_subtitle: "PGS",
  dvd_subtitle: "VobSub",
  dvb_subtitle: "DVB",
};

/** Subtitles made of pictures, read with Windows' OCR rather than as text. */
const PICTURES = new Set(["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle"]);

/** The encodings a text subtitle file not in UTF-8 may be read in (`local/mkv/added.rs`), as people know them. */
const CHARSETS: { id: string; label: string }[] = [
  { id: "CP1253", label: "Greek (Windows-1253)" },
  { id: "ISO-8859-7", label: "Greek (ISO-8859-7)" },
  { id: "CP1252", label: "Western (Windows-1252)" },
  { id: "CP1250", label: "Central European (Windows-1250)" },
  { id: "CP1251", label: "Cyrillic (Windows-1251)" },
  { id: "CP1254", label: "Turkish (Windows-1254)" },
  { id: "CP1257", label: "Baltic (Windows-1257)" },
  { id: "CP1255", label: "Hebrew (Windows-1255)" },
  { id: "CP1256", label: "Arabic (Windows-1256)" },
  { id: "CP1258", label: "Vietnamese (Windows-1258)" },
  { id: "CP874", label: "Thai (Windows-874)" },
  { id: "SHIFT_JIS", label: "Japanese (Shift JIS)" },
  { id: "GBK", label: "Chinese, simplified (GBK)" },
  { id: "BIG5", label: "Chinese, traditional (Big5)" },
  { id: "CP949", label: "Korean (Windows-949)" },
];

/** The language Select's value for "not known". */
const UNKNOWN = "und";

const codec = (name: string) => CODECS[name] ?? (name.startsWith("pcm_") ? "PCM" : name.toUpperCase());

function channels(count: number | null): string | null {
  if (count === null) return null;
  return { 1: "Mono", 2: "Stereo", 6: "5.1", 8: "7.1" }[count] ?? `${count} channels`;
}

/**
 * An MKV's tracks, to choose which it keeps and what the copy says of each:
 * its language and how Siphon knows it, its name, its flags, its place.
 */
export function TracksPanel({
  fileKey,
  version,
  draft,
  onDraft,
  disabled,
  embedded = false,
}: {
  fileKey: string;
  version: string;
  /** What has been chosen so far; null until the file is read. */
  draft: Draft | null;
  onDraft: (draft: Draft) => void;
  /** A copy is being made: nothing can change meanwhile. */
  disabled: boolean;
  /** One file of a season, opened in its list: the season says what the languages did, and what saving makes. */
  embedded?: boolean;
}) {
  const languages = useLanguagesKey();
  const contents = useQuery(tracksQuery(fileKey, version, languages));
  const data = contents.data;
  const current =
    draft !== null && draft.key === fileKey && draft.version === version && draft.languages === languages;
  // Files added are read a moment after they are picked: their tracks go in the draft as it is then.
  const latest = useRef(draft);
  latest.current = draft;
  useEffect(() => {
    if (data && !current && languages !== null) onDraft(draftOf(data, fileKey, version, languages));
  }, [data, current, fileKey, version, languages, onDraft]);

  if (contents.isError) return <ErrorCard error={toAppError(contents.error)} />;
  if (!data || !current) {
    return (
      <div aria-busy className="space-y-2 px-1 py-1.5">
        <p className="text-xs text-muted-foreground">Reading its tracks and their subtitles…</p>
        {[0, 1, 2].map((row) => (
          <div key={row} aria-hidden className="skeleton h-9 rounded-xl" />
        ))}
      </div>
    );
  }

  const { chapters, doubts } = data;

  return (
    <fieldset disabled={disabled} className="min-w-0 space-y-3 border-0 p-0 py-1.5">
      {data.suggestion && !embedded && <Languages suggestion={data.suggestion} draft={draft} onDraft={onDraft} />}
      {KINDS.map(({ kind, label, icon }) => {
        const ofKind = draft.tracks.filter((track) => track.kind === kind);
        if (ofKind.length === 0) return null;
        return (
          <section key={kind} aria-label={label}>
            <p className="px-1 pb-1 text-xs font-medium text-subtle-foreground">{label}</p>
            <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
              {ofKind.map((track, place) => {
                const read = readOf(draft, data, track);
                const added = addedOf(draft, track);
                return read ? (
                  <TrackRow
                    key={track.index}
                    track={track}
                    read={read}
                    added={added}
                    onCharset={
                      added
                        ? (charset) =>
                            void ipc
                              .readAdded(added.key, charset)
                              .then((file) => latest.current && onDraft(addTracks(latest.current, file)))
                        : undefined
                    }
                    icon={icon}
                    first={place === 0}
                    last={place === ofKind.length - 1}
                    onChange={(change) => onDraft(withTrack(draft, track.index, change))}
                    onMove={(by) => onDraft(moved(draft, track.index, by))}
                  />
                ) : null;
              })}
            </ul>
          </section>
        );
      })}

      <ChaptersPart draft={draft} contents={data} onDraft={onDraft} />
      <AttachedPart draft={draft} contents={data} onDraft={onDraft} />

      <AddTracks onAdded={(reads) => latest.current && onDraft(reads.reduce(applyPicked, latest.current))} />

      <AnimatePresence initial={false}>
        {doubts.map((doubt) => (
          <Note key={doubt} warning>
            {doubt}
          </Note>
        ))}
        {chapters.note && (
          <Note key="chapters" warning={false}>
            {chapters.note}
          </Note>
        )}
      </AnimatePresence>

      <KindOfCopy draft={draft} contents={data} onDraft={onDraft} />

      {!embedded && <Summary draft={draft} contents={data} />}
    </fieldset>
  );
}

/**
 * What the user's languages did to the file, said plainly, with the way back:
 * what they left out and why, what plays first now, and what was kept for
 * not being certain.
 */
function Languages({
  suggestion,
  draft,
  onDraft,
}: {
  suggestion: Suggestion;
  draft: Draft;
  onDraft: (draft: Draft) => void;
}) {
  const openPanel = useUi((state) => state.openPanel);
  const names = suggestion.languages.map((language) => languageName(language));
  const yours = names.length > 0 ? names.join(", ") : "none chosen";
  const said = [...suggestion.left_out.map((left) => left.why), ...suggestion.notes];
  const settingsLink = (
    <button
      type="button"
      onClick={() => openPanel("settings")}
      className="rounded-sm text-primary underline-offset-2 hover:underline focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
    >
      Settings
    </button>
  );
  const action = (label: string, onClick: () => void) => (
    <button
      type="button"
      onClick={onClick}
      className="-my-1 h-7 shrink-0 rounded-lg bg-background/70 px-2.5 text-xs font-medium text-foreground transition-colors hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
    >
      {label}
    </button>
  );

  if (said.length === 0) {
    return (
      <p className="px-1 text-xs text-muted-foreground">
        Everything in it is in your languages ({yours}). Your languages are in {settingsLink}.
      </p>
    );
  }
  return (
    <div className="flex items-start gap-3 rounded-xl bg-secondary px-3 py-2.5 text-[13px] text-muted-foreground">
      <div className="min-w-0 flex-1 space-y-1">
        <p className="text-foreground">
          {draft.mine ? `Kept for your languages: ${yours}.` : "Kept as the file has it."}{" "}
          <span className="text-muted-foreground">Your languages are in {settingsLink}.</span>
        </p>
        {draft.mine && (
          <ul className="space-y-0.5">
            {said.map((line) => (
              <li key={line} className="text-pretty">
                {line}
              </li>
            ))}
          </ul>
        )}
      </div>
      {draft.mine
        ? action("Keep everything", () => onDraft(asItIs(draft)))
        : action("Keep my languages", () => onDraft(mineOf(draft, suggestion.edit)))}
    </div>
  );
}

/** What saving makes, in a line: how many tracks it keeps, and about how large it is. */
function Summary({ draft, contents }: { draft: Draft; contents: MkvContents }) {
  const { kept, removed, added, changed } = changesOf(draft);
  const size = sizeOf(draft, contents);
  const adds = added > 0 ? `, adds ${added}` : "";
  const headline = !changed
    ? "Keeps it as it is"
    : removed > 0
      ? `Keeps ${kept} of ${draft.before.length} tracks${adds}`
      : added > 0
        ? `Keeps every track${adds}`
        : "Keeps every track, changed";
  return (
    <div className="flex items-center gap-3 px-1 pt-1">
      <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary-soft text-primary">
        <Zap className="size-4" />
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium">{headline}</p>
        <p className="text-xs text-muted-foreground">
          <span className="text-primary">No quality loss</span> · Every track copied as it is
        </p>
      </div>
      {size !== null && (
        <span className="shrink-0 text-[15px] font-medium tabular-nums">
          <span className="text-muted-foreground">≈ </span>
          {formatBytes(size)}
        </span>
      )}
    </div>
  );
}

/** What a file picked to add does to a draft: its tracks added, its chapters taken, a font attached. */
export function applyPicked(draft: Draft, read: PickedRead): Draft {
  switch (read.kind) {
    case "tracks":
      return addTracks(draft, read.added);
    case "chapters":
      return withChapters(draft, read.chapters, read.file.name);
    case "font":
      return withFonts(draft, [read.file]);
  }
}

/**
 * Adding to the copy from other files (docs/mkv-editor.md, steps 6 and 7):
 * subtitles, sound or another video's tracks, a chapters file, fonts;
 * picked in Windows' file dialog and read before they go in.
 */
function AddTracks({ onAdded }: { onAdded: (reads: PickedRead[]) => void }) {
  const [reading, setReading] = useState<string[]>([]);
  const [failed, setFailed] = useState<string[]>([]);
  const add = async () => {
    const picked = await ipc.pickTrackFiles().catch(() => null);
    if (!picked || picked.length === 0) return;
    setFailed([]);
    setReading(picked.map((file) => file.name));
    const reads = await Promise.allSettled(picked.map(readPicked));
    setReading([]);
    setFailed(reads.flatMap((read) => (read.status === "rejected" ? [toAppError(read.reason).message] : [])));
    const done = reads.flatMap((read) => (read.status === "fulfilled" ? [read.value] : []));
    if (done.length > 0) onAdded(done);
  };
  return (
    <div className="space-y-2">
      <button
        type="button"
        onClick={() => void add()}
        disabled={reading.length > 0}
        className="flex h-12 w-full items-center gap-2.5 rounded-xl border-[1.5px] border-dashed border-border-strong px-3.5 text-left text-[13.5px] text-muted-foreground transition-colors outline-none hover:border-primary/50 hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:cursor-progress"
      >
        <Plus className="size-4 shrink-0" />
        <span className="min-w-0 truncate">
          {reading.length > 0
            ? `Reading ${reading.length === 1 ? reading[0] : `${reading.length} files`}…`
            : "Add subtitles, sound, chapters or fonts"}
        </span>
      </button>
      <AnimatePresence initial={false}>
        {failed.map((why) => (
          <Note key={why} warning>
            {why}
          </Note>
        ))}
      </AnimatePresence>
    </div>
  );
}

/**
 * What kind of file the copy is, for a file that isn't an MKV: an MP4's
 * stays an MP4, unless it is to hold what an MP4 can't (pictures for
 * subtitles, fonts…) or the user asks; an MPEG-TS's becomes an MKV.
 */
function KindOfCopy({
  draft,
  contents,
  onDraft,
}: {
  draft: Draft;
  contents: MkvContents;
  onDraft: (draft: Draft) => void;
}) {
  if (draft.container === "matroska" || draft.container === "other") return null;
  if (draft.container === "ts") {
    return <p className="px-1 text-xs text-muted-foreground">The copy is an MKV, which keeps every track as it is.</p>;
  }
  const misfits = draft.tracks.flatMap((track) => {
    if (!track.keep) return [];
    const read = readOf(draft, contents, track);
    return read && !fitsMp4(read) ? [read] : [];
  });
  const needsMkv = misfits.length > 0 || draft.fonts.length > 0;
  const why = [
    misfits.length > 0 && `its ${[...new Set(misfits.map((track) => codec(track.codec)))].join(" and ")} tracks`,
    draft.fonts.length > 0 && "fonts",
  ]
    .filter(Boolean)
    .join(" or ");
  return (
    <div className="flex items-center gap-2 px-1 text-xs text-muted-foreground">
      <p className="min-w-0 flex-1 text-pretty">
        {savedAsMkv(draft)
          ? needsMkv
            ? `The copy is an MKV: an MP4 can’t hold ${why}.`
            : "The copy is an MKV."
          : "The copy stays an MP4. Its subtitles are written as an MP4’s timed text."}
      </p>
      {savedAsMkv(draft) && !needsMkv && (
        <SectionAction onClick={() => onDraft({ ...draft, mkv: false })}>Keep it an MP4</SectionAction>
      )}
      {!savedAsMkv(draft) && <SectionAction onClick={() => onDraft({ ...draft, mkv: true })}>Save it as an MKV</SectionAction>}
    </div>
  );
}

/** A small action beside a section's name. */
function SectionAction({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="rounded-md px-1.5 py-0.5 text-xs text-primary transition-colors hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
    >
      {children}
    </button>
  );
}

/** A chapter list longer than this shows its first ones, and the rest on asking. */
const CHAPTERS_SHOWN = 5;

/**
 * The copy's chapters (docs/mkv-editor.md, step 7): each renamed or left
 * out, all left out, or taken from a chapters file added.
 */
function ChaptersPart({
  draft,
  contents,
  onDraft,
}: {
  draft: Draft;
  contents: MkvContents;
  onDraft: (draft: Draft) => void;
}) {
  const [all, setAll] = useState(false);
  const own = contents.chapters.list;
  if (own.length === 0 && draft.chapterList === null) return null;
  const list = chaptersOf(draft, contents);
  const kept = draft.chapters || draft.chapterList !== null;
  const shown = all ? list : list.slice(0, CHAPTERS_SHOWN);
  return (
    <section aria-label="Chapters">
      <div className="flex items-center gap-2 px-1 pb-1">
        <p className="min-w-0 flex-1 truncate text-xs font-medium text-subtle-foreground">
          Chapters
          {kept && list.length > 0 && <span className="tabular-nums"> · {list.length}</span>}
          {draft.chaptersFrom && <span className="font-normal"> · from {draft.chaptersFrom}</span>}
        </p>
        {draft.chapterList !== null && own.length > 0 && (
          <SectionAction onClick={() => onDraft({ ...draft, chapters: true, chapterList: null, chaptersFrom: null })}>
            As the file has them
          </SectionAction>
        )}
        {kept && list.length > 0 ? (
          <SectionAction onClick={() => onDraft({ ...draft, chapters: false, chapterList: null, chaptersFrom: null })}>
            Leave them out
          </SectionAction>
        ) : (
          !kept && <SectionAction onClick={() => onDraft({ ...draft, chapters: true })}>Keep them</SectionAction>
        )}
      </div>
      {kept && list.length > 0 ? (
        <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
          {shown.map((chapter, place) => (
            <li key={`${chapter.start}-${place}`} className="flex items-center gap-2 py-1 pr-2 pl-3">
              <span className="w-16 shrink-0 text-xs text-muted-foreground tabular-nums">
                {formatDuration(chapter.start)}
              </span>
              <input
                aria-label={`Chapter ${place + 1}’s name`}
                placeholder="No name"
                value={chapter.title}
                onChange={(event) =>
                  onDraft(
                    withChapters(
                      draft,
                      list.map((other, at) => (at === place ? { ...other, title: event.target.value } : other)),
                    ),
                  )
                }
                className="h-8 min-w-0 flex-1 rounded-lg bg-transparent px-2 text-[13px] outline-none transition-colors placeholder:text-subtle-foreground hover:bg-background/50 focus-visible:bg-background/70 focus-visible:ring-[3px] focus-visible:ring-ring/40"
              />
              <Hint label="Leave this chapter out">
                <button
                  type="button"
                  aria-label={`Leave chapter ${place + 1} out`}
                  onClick={() => onDraft(withChapters(draft, list.filter((_, at) => at !== place)))}
                  className="grid size-7 shrink-0 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
                >
                  <X className="size-3.5" />
                </button>
              </Hint>
            </li>
          ))}
          {list.length > CHAPTERS_SHOWN && (
            <li>
              <button
                type="button"
                onClick={() => setAll(!all)}
                className="w-full px-3 py-2 text-left text-xs text-primary hover:bg-accent"
              >
                {all ? "Show fewer" : `Show all ${list.length}`}
              </button>
            </li>
          )}
        </ul>
      ) : (
        <p className="rounded-xl bg-secondary/50 px-3 py-2.5 text-xs text-muted-foreground">
          Left out: the copy has no chapters.
        </p>
      )}
    </section>
  );
}

/** "Font", "Picture": what an attached file is, as people say it. */
function attachmentKind(mime: string | null, name: string): { label: string; icon: typeof Type } {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (mime?.startsWith("image/")) return { label: "Picture", icon: Image };
  if (mime?.includes("font") || mime?.includes("opentype") || ["ttf", "otf", "ttc"].includes(ext)) {
    return { label: "Font, for its ASS subtitles", icon: Type };
  }
  return { label: "File", icon: FileIcon };
}

/**
 * The files attached to the copy (docs/mkv-editor.md, step 7): the MKV's
 * own, each kept or left out, and fonts added for ASS subtitles.
 */
function AttachedPart({
  draft,
  contents,
  onDraft,
}: {
  draft: Draft;
  contents: MkvContents;
  onDraft: (draft: Draft) => void;
}) {
  const own: Attachment[] = contents.attachments;
  if (own.length === 0 && draft.fonts.length === 0) return null;
  return (
    <section aria-label="Attached files">
      <p className="px-1 pb-1 text-xs font-medium text-subtle-foreground">Attached files</p>
      <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
        {own.map((file) => {
          const kept = draft.attachments && !draft.leftOutAttachments.includes(file.index);
          const { label, icon: Icon } = attachmentKind(file.mime, file.name);
          return (
            <li key={file.index} className={cn("flex items-center gap-3 px-3 py-2 transition-opacity", !kept && "opacity-55")}>
              <button
                type="button"
                role="checkbox"
                aria-checked={kept}
                aria-label={`Keep ${file.name}`}
                onClick={() =>
                  onDraft({
                    ...draft,
                    attachments: true,
                    leftOutAttachments: kept
                      ? [...draft.leftOutAttachments, file.index]
                      : draft.leftOutAttachments.filter((index) => index !== file.index),
                  })
                }
                className={cn(
                  "grid size-5 shrink-0 place-items-center rounded-md border transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
                  kept
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-border-strong bg-card text-transparent hover:border-primary",
                )}
              >
                <Check className="size-3.5" strokeWidth={3} />
              </button>
              <Icon className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <p className={cn("truncate text-sm font-medium", !kept && "line-through")}>{file.name}</p>
                <p className="truncate text-xs text-muted-foreground">{label}</p>
              </div>
            </li>
          );
        })}
        {draft.fonts.map((font) => (
          <li key={font.key} className="flex items-center gap-3 px-3 py-2">
            <span className="grid size-5 shrink-0 place-items-center rounded-md bg-primary text-primary-foreground">
              <Check className="size-3.5" strokeWidth={3} />
            </span>
            <Type className="size-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium">{font.name}</p>
              <p className="truncate text-xs text-muted-foreground">Font, for its ASS subtitles</p>
            </div>
            <span className="shrink-0 rounded-md bg-background/70 px-1.5 py-0.5 text-[11px] text-muted-foreground">
              Added
            </span>
            <Hint label="Don’t attach it">
              <button
                type="button"
                aria-label={`Don’t attach ${font.name}`}
                onClick={() => onDraft({ ...draft, fonts: draft.fonts.filter((other) => other.key !== font.key) })}
                className="grid size-7 shrink-0 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            </Hint>
          </li>
        ))}
      </ul>
    </section>
  );
}

function TrackRow({
  track,
  read,
  added,
  onCharset,
  icon: Icon,
  first,
  last,
  onChange,
  onMove,
}: {
  track: DraftTrack;
  /** What reading the file found of it. */
  read: MkvTrack;
  /** The file it was added from, if it was. */
  added?: DraftAdded;
  /** Reads the file it was added from again, in another encoding. */
  onCharset?: (charset: string) => void;
  icon: typeof Film;
  first: boolean;
  last: boolean;
  onChange: (change: Partial<DraftTrack>) => void;
  onMove: (by: -1 | 1) => void;
}) {
  const [open, setOpen] = useState(false);
  const video = track.kind === "video";
  const title = video
    ? read.height
      ? `${Math.min(read.width ?? read.height, read.height)}p`
      : "Video"
    : track.language
      ? languageName(track.language)
      : "Language unknown";
  const details = [
    codec(read.codec),
    track.kind === "sound" ? channels(read.channels) : null,
    track.name ? `“${track.name}”` : null,
    added ? `from ${added.name}` : null,
  ].filter((detail) => detail !== null);
  const flags = [
    // Which video plays first decides nothing: a file has one picture.
    !video && track.default && "Plays first",
    track.forced && "Forced",
    track.sdh && "SDH",
    track.commentary && "Commentary",
    track.described && "Described",
    track.original && "Original",
    added ? "Added" : false,
  ].filter((flag): flag is string => typeof flag === "string");

  return (
    <motion.li layout transition={spring}>
      <div className={cn("flex items-center gap-3 px-3 py-2 transition-opacity", !track.keep && "opacity-55")}>
        <button
          type="button"
          role="checkbox"
          aria-checked={track.keep}
          aria-label={`Keep ${title}`}
          onClick={() => onChange({ keep: !track.keep })}
          className={cn(
            "grid size-5 shrink-0 place-items-center rounded-md border transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
            track.keep
              ? "border-primary bg-primary text-primary-foreground"
              : "border-border-strong bg-card text-transparent hover:border-primary",
          )}
        >
          <Check className="size-3.5" strokeWidth={3} />
        </button>
        <Icon className="size-4 shrink-0 text-muted-foreground" />
        <button
          type="button"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          className="min-w-0 flex-1 rounded-md text-left focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
        >
          <p
            className={cn(
              "truncate text-sm font-medium",
              !video && !track.language && "text-muted-foreground",
              !track.keep && "line-through",
            )}
          >
            {title}
          </p>
          <p className="truncate text-xs text-muted-foreground">{details.join(" · ")}</p>
        </button>
        {flags.map((flag) => (
          <span key={flag} className="shrink-0 rounded-md bg-background/70 px-1.5 py-0.5 text-[11px] text-muted-foreground">
            {flag}
          </span>
        ))}
        {!video && <Certainty track={track} read={read} />}
        <button
          type="button"
          aria-label={open ? "Close its details" : "Its language, name and flags"}
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="grid size-6 shrink-0 place-items-center rounded-full text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <ChevronDown className={cn("size-3.5 transition-transform", open && "rotate-180")} />
        </button>
      </div>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={spring}
            className="overflow-hidden"
          >
            <TrackDetails
              track={track}
              added={added}
              onCharset={onCharset}
              first={first}
              last={last}
              onChange={onChange}
              onMove={onMove}
            />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** A track's language, name, flags and place, as the copy will have them. */
function TrackDetails({
  track,
  added,
  onCharset,
  first,
  last,
  onChange,
  onMove,
}: {
  track: DraftTrack;
  added?: DraftAdded;
  onCharset?: (charset: string) => void;
  first: boolean;
  last: boolean;
  onChange: (change: Partial<DraftTrack>) => void;
  onMove: (by: -1 | 1) => void;
}) {
  const video = track.kind === "video";
  const mine = systemLanguage();
  const languages = [...new Set([...(track.language ? [track.language] : []), ...COMMON_LANGUAGES])].sort(
    (a, b) =>
      Number(primary(b) === mine) - Number(primary(a) === mine) || languageName(a).localeCompare(languageName(b)),
  );
  const toggles: { label: string; on: boolean; change: Partial<DraftTrack> }[] =
    track.kind === "sound"
      ? [
          { label: "Plays first", on: track.default, change: { default: !track.default } },
          { label: "Original", on: track.original, change: { original: !track.original } },
          { label: "Commentary", on: track.commentary, change: { commentary: !track.commentary } },
        ]
      : track.kind === "subtitles"
        ? [
            { label: "Plays first", on: track.default, change: { default: !track.default } },
            { label: "Forced", on: track.forced, change: { forced: !track.forced } },
            { label: "SDH", on: track.sdh, change: { sdh: !track.sdh } },
          ]
        : [];

  return (
    <div className="flex flex-wrap items-center gap-2 px-3 pb-3 pl-[4.25rem]">
      {added && added.sample.length > 0 && (
        <p className="w-full truncate text-xs text-muted-foreground italic" title={added.sample.join("\n")}>
          “{added.sample[0]?.replace(/\n/g, " ")}”
        </p>
      )}
      {added?.charset && onCharset && (
        <Select value={added.charset} onValueChange={onCharset}>
          <SelectTrigger aria-label="Read as" className="h-8 w-72">
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="max-h-72">
            {CHARSETS.map((charset) => (
              <SelectItem key={charset.id} value={charset.id}>
                Read as {charset.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {!video && (
        <Select
          value={track.language ?? UNKNOWN}
          onValueChange={(value) => onChange({ language: value === UNKNOWN ? null : value })}
        >
          <SelectTrigger aria-label="Language" className="h-8 w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="max-h-72">
            {!track.language && <SelectItem value={UNKNOWN}>Language unknown</SelectItem>}
            {languages.map((language) => (
              <SelectItem key={language} value={language}>
                {languageName(language)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      <input
        aria-label="Name"
        placeholder="Name (optional)"
        value={track.name}
        onChange={(event) => onChange({ name: event.target.value })}
        className="h-8 w-44 min-w-0 rounded-lg border border-border-strong bg-card px-3 text-[13px] outline-none transition-shadow placeholder:text-subtle-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40"
      />
      {toggles.map((toggle) => (
        <FlagToggle key={toggle.label} on={toggle.on} onClick={() => onChange(toggle.change)}>
          {toggle.label}
        </FlagToggle>
      ))}
      {!(first && last) && (
        <span className="ml-auto flex gap-1">
          <Hint label="Move up">
            <button type="button" aria-label="Move up" disabled={first} onClick={() => onMove(-1)} className={MOVE}>
              <ArrowUp className="size-3.5" />
            </button>
          </Hint>
          <Hint label="Move down">
            <button type="button" aria-label="Move down" disabled={last} onClick={() => onMove(1)} className={MOVE}>
              <ArrowDown className="size-3.5" />
            </button>
          </Hint>
        </span>
      )}
    </div>
  );
}

const MOVE =
  "grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-35";

/** How Siphon knows a track's language: read, taken by its tag, or in doubt. */
function Certainty({ track, read }: { track: DraftTrack; read: MkvTrack }) {
  const language = read.language ? languageName(read.language) : null;
  const how = PICTURES.has(read.codec) ? "Windows read its pictures" : "Siphon read its words";
  let mark: ReactNode;
  let says: string[];
  if (!read.certain) {
    mark = <TriangleAlert className="size-3.5 text-destructive" />;
    says = read.doubts.length > 0 ? read.doubts : ["Its language isn’t known."];
    says = [...says, "Siphon leaves out only what it is sure of: whether it goes is yours to say."];
  } else if (read.read) {
    mark = <Check className="size-3.5 text-primary" />;
    says = [`${how}: ${language}.`];
  } else {
    mark = <Tag className="size-3.5 text-subtle-foreground" />;
    says = [`${language}, as the file says.`];
  }
  if (track.language !== read.language) {
    says = [...says, `The copy says ${track.language ? languageName(track.language) : "nothing of its language"}.`];
  }
  return (
    <Hint
      label={
        <span className="block space-y-1.5">
          {[...says, ...read.notes].map((line) => (
            <span key={line} className="block">
              {line}
            </span>
          ))}
        </span>
      }
      className="max-w-80 px-3 py-2 leading-relaxed text-pretty"
    >
      <button
        type="button"
        aria-label={read.certain ? `How Siphon knows it is ${language}` : "Why this track is in doubt"}
        className="grid size-6 shrink-0 place-items-center rounded-full transition-colors hover:bg-accent"
      >
        {mark}
      </button>
    </Hint>
  );
}
