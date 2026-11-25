import { useQuery } from "@tanstack/react-query";
import { Captions, Check, Film, Tag, TriangleAlert, Volume2 } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Hint } from "@/components/ui/tooltip";
import { Note } from "@/features/options/PlanCard";
import { toAppError, ipc } from "@/lib/ipc";
import { languageName } from "@/lib/languages";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { MkvKind, MkvTrack } from "@/types";

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

const codec = (name: string) => CODECS[name] ?? (name.startsWith("pcm_") ? "PCM" : name.toUpperCase());

function channels(count: number | null): string | null {
  if (count === null) return null;
  return { 1: "Mono", 2: "Stereo", 6: "5.1", 8: "7.1" }[count] ?? `${count} channels`;
}

/** What each track of an MKV is: its language and how Siphon knows it, its codec and flags. */
export function TracksPanel({ fileKey, version }: { fileKey: string; version: string }) {
  const contents = useQuery({
    queryKey: ["tracks", fileKey, version],
    queryFn: () => ipc.readTracks(fileKey),
    staleTime: Number.POSITIVE_INFINITY,
  });

  if (contents.isError) return <ErrorCard error={toAppError(contents.error)} />;
  if (!contents.data) {
    return (
      <div aria-busy className="space-y-2 px-1 py-1.5">
        <p className="text-xs text-muted-foreground">Reading its tracks and their subtitles…</p>
        {[0, 1, 2].map((row) => (
          <div key={row} aria-hidden className="skeleton h-9 rounded-xl" />
        ))}
      </div>
    );
  }

  const { tracks, chapters, attachments, doubts } = contents.data;
  const extras = [
    chapters.count > 0 ? `${chapters.count} chapter${chapters.count === 1 ? "" : "s"}` : null,
    attachments.length > 0 ? `${attachments.length} attached file${attachments.length === 1 ? "" : "s"}` : null,
  ].filter((extra) => extra !== null);

  return (
    <div className="space-y-3 py-1.5">
      {KINDS.map(({ kind, label, icon }) => {
        const ofKind = tracks.filter((track) => track.kind === kind);
        if (ofKind.length === 0) return null;
        return (
          <section key={kind} aria-label={label}>
            <p className="px-1 pb-1 text-xs font-medium text-subtle-foreground">{label}</p>
            <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
              {ofKind.map((track) => (
                <TrackRow key={track.index} track={track} icon={icon} />
              ))}
            </ul>
          </section>
        );
      })}

      {extras.length > 0 && (
        <p className="px-1 text-xs text-muted-foreground">{extras.join(" · ")}</p>
      )}

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
    </div>
  );
}

function TrackRow({ track, icon: Icon }: { track: MkvTrack; icon: typeof Film }) {
  const video = track.kind === "video";
  const title = video
    ? track.height
      ? `${Math.min(track.width ?? track.height, track.height)}p`
      : "Video"
    : track.language
      ? languageName(track.language)
      : "Language unknown";
  const details = [
    codec(track.codec),
    track.kind === "sound" ? channels(track.channels) : null,
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
    <motion.li layout transition={spring} className="flex items-center gap-3 px-3 py-2">
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1">
        <p className={cn("truncate text-sm font-medium", !video && !track.language && "text-muted-foreground")}>
          {title}
        </p>
        <p className="truncate text-xs text-muted-foreground">{details.join(" · ")}</p>
      </div>
      {flags.map((flag) => (
        <span key={flag} className="shrink-0 rounded-md bg-background/70 px-1.5 py-0.5 text-[11px] text-muted-foreground">
          {flag}
        </span>
      ))}
      {!video && <Certainty track={track} />}
    </motion.li>
  );
}

/** How Siphon knows a track's language: read, taken by its tag, or in doubt. */
function Certainty({ track }: { track: MkvTrack }) {
  const language = track.language ? languageName(track.language) : null;
  const how = PICTURES.has(track.codec) ? "Windows read its pictures" : "Siphon read its words";
  let mark: ReactNode;
  let says: string[];
  if (!track.certain) {
    mark = <TriangleAlert className="size-3.5 text-destructive" />;
    says = track.doubts.length > 0 ? track.doubts : ["Its language isn’t known."];
    says = [...says, "It is kept: Siphon leaves out only what it is sure of."];
  } else if (track.read) {
    mark = <Check className="size-3.5 text-primary" />;
    says = [`${how}: ${language}.`];
  } else {
    mark = <Tag className="size-3.5 text-subtle-foreground" />;
    says = [`${language}, as the file says.`];
  }
  return (
    <Hint
      label={
        <span className="block space-y-1.5">
          {[...says, ...track.notes].map((line) => (
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
        aria-label={track.certain ? `How Siphon knows it is ${language}` : "Why this track is in doubt"}
        className="grid size-6 shrink-0 place-items-center rounded-full transition-colors hover:bg-accent"
      >
        {mark}
      </button>
    </Hint>
  );
}
