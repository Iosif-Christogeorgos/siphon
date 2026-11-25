import { useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Captions, Check, ChevronDown, Film, Tag, TriangleAlert, Volume2, Zap } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Hint } from "@/components/ui/tooltip";
import { Note } from "@/features/options/PlanCard";
import { formatBytes } from "@/lib/format";
import { ipc, toAppError } from "@/lib/ipc";
import { languageName, primary, systemLanguage } from "@/lib/languages";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { MkvContents, MkvKind, MkvTrack } from "@/types";
import { changesOf, type Draft, type DraftTrack, draftOf, moved, sizeOf, withTrack } from "./draft";

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

/** The languages a track can be said to be in, beside the one it is in. */
const LANGUAGES = [
  "ar", "bg", "cs", "da", "de", "el", "en", "es", "fa", "fi", "fr", "he", "hi", "hr", "hu", "id", "it", "ja",
  "ko", "ms", "nl", "no", "pl", "pt", "ro", "ru", "sk", "sl", "sr", "sv", "th", "tr", "uk", "vi", "zh",
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
}: {
  fileKey: string;
  version: string;
  /** What has been chosen so far; null until the file is read. */
  draft: Draft | null;
  onDraft: (draft: Draft) => void;
  /** A copy is being made: nothing can change meanwhile. */
  disabled: boolean;
}) {
  const contents = useQuery({
    queryKey: ["tracks", fileKey, version],
    queryFn: () => ipc.readTracks(fileKey),
    staleTime: Number.POSITIVE_INFINITY,
  });
  const data = contents.data;
  const current = draft !== null && draft.key === fileKey && draft.version === version;
  useEffect(() => {
    if (data && !current) onDraft(draftOf(data, fileKey, version));
  }, [data, current, fileKey, version, onDraft]);

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

  const { chapters, attachments, doubts } = data;
  const extras = [
    chapters.count > 0 ? `${chapters.count} chapter${chapters.count === 1 ? "" : "s"}` : null,
    attachments.length > 0 ? `${attachments.length} attached file${attachments.length === 1 ? "" : "s"}` : null,
  ].filter((extra) => extra !== null);

  return (
    <fieldset disabled={disabled} className="min-w-0 space-y-3 border-0 p-0 py-1.5">
      {KINDS.map(({ kind, label, icon }) => {
        const ofKind = draft.tracks.filter((track) => track.kind === kind);
        if (ofKind.length === 0) return null;
        return (
          <section key={kind} aria-label={label}>
            <p className="px-1 pb-1 text-xs font-medium text-subtle-foreground">{label}</p>
            <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
              {ofKind.map((track, place) => {
                const read = data.tracks.find((candidate) => candidate.index === track.index);
                return read ? (
                  <TrackRow
                    key={track.index}
                    track={track}
                    read={read}
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

      {extras.length > 0 && <p className="px-1 text-xs text-muted-foreground">{extras.join(" · ")}</p>}

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

      <Summary draft={draft} contents={data} />
    </fieldset>
  );
}

/** What saving makes, in a line: how many tracks it keeps, and about how large it is. */
function Summary({ draft, contents }: { draft: Draft; contents: MkvContents }) {
  const { kept, removed, changed } = changesOf(draft);
  const size = sizeOf(draft, contents);
  const headline = !changed
    ? "Keeps it as it is"
    : removed > 0
      ? `Keeps ${kept} of ${draft.tracks.length} tracks`
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

function TrackRow({
  track,
  read,
  icon: Icon,
  first,
  last,
  onChange,
  onMove,
}: {
  track: DraftTrack;
  /** What reading the file found of it. */
  read: MkvTrack;
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
  ].filter((detail) => detail !== null);
  const flags = [
    // Which video plays first decides nothing: a file has one picture.
    !video && track.default && "Plays first",
    track.forced && "Forced",
    track.sdh && "SDH",
    track.commentary && "Commentary",
    track.described && "Described",
    track.original && "Original",
  ].filter((flag) => flag !== false);

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
            <TrackDetails track={track} first={first} last={last} onChange={onChange} onMove={onMove} />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** A track's language, name, flags and place, as the copy will have them. */
function TrackDetails({
  track,
  first,
  last,
  onChange,
  onMove,
}: {
  track: DraftTrack;
  first: boolean;
  last: boolean;
  onChange: (change: Partial<DraftTrack>) => void;
  onMove: (by: -1 | 1) => void;
}) {
  const video = track.kind === "video";
  const mine = systemLanguage();
  const languages = [...new Set([...(track.language ? [track.language] : []), ...LANGUAGES])].sort(
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
        placeholder="No name"
        value={track.name}
        onChange={(event) => onChange({ name: event.target.value })}
        className="h-8 w-44 min-w-0 rounded-lg border border-border-strong bg-card px-3 text-[13px] outline-none transition-shadow placeholder:text-subtle-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40"
      />
      {toggles.map((toggle) => (
        <Toggle key={toggle.label} on={toggle.on} onClick={() => onChange(toggle.change)}>
          {toggle.label}
        </Toggle>
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

function Toggle({ on, onClick, children }: { on: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={cn(
        "h-8 rounded-lg px-2.5 text-xs transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
        on ? "bg-primary-soft text-primary" : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

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
