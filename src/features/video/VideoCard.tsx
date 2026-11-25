import { AudioLines, ChevronRight, ImageOff, LogIn, X } from "lucide-react";
import { motion } from "motion/react";
import { type ReactNode, useState } from "react";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatCount, formatDuration, formatSpan } from "@/lib/format";
import { AUDIO_CODEC_LABELS, bestVideoFormat, resolutionLabel, VIDEO_CODEC_LABELS } from "@/lib/formats";
import { useNow } from "@/lib/time";
import { displayHost } from "@/lib/url";
import { cn } from "@/lib/utils";
import type { Format, Live, VideoInfo } from "@/types";

/** "C:\Users\Ana\Videos" → "Videos": where a file is, in a word. */
const folderName = (folder: string) => folder.split(/[\\/]/).filter(Boolean).at(-1) ?? folder;

/** "H.264 + AAC", "MP3": what a file holds. */
function codecs(format: Format | undefined): string | null {
  const parts = [
    format?.video_codec ? VIDEO_CODEC_LABELS[format.video_codec] : null,
    format?.audio_codec ? AUDIO_CODEC_LABELS[format.audio_codec] : null,
  ].filter(Boolean);
  return parts.length > 0 ? parts.join(" + ") : null;
}

/** An MKV's tracks, offered as a chip among the card's: opens them to choose which it keeps. */
export interface TracksChip {
  count: number;
  open: boolean;
  onToggle: () => void;
  disabled: boolean;
}

export function VideoCard({ video, action, tracks }: { video: VideoInfo; action?: ReactNode; tracks?: TracksChip }) {
  const best = bestVideoFormat(video.formats);
  const quality = best ? resolutionLabel(best) : null;
  const file = video.file;
  const byline = file
    ? `${folderName(file.folder)} · ${formatBytes(file.size)}`
    : [video.channel, displayHost(video.url)].filter(Boolean).join(" · ");

  return (
    <article className="group/card flex gap-4 rounded-[1.25rem] border bg-card p-3 shadow-card">
      <Thumbnail
        src={video.thumbnail}
        duration={video.duration}
        live={video.live}
        sound={file !== undefined && video.formats[0]?.kind === "audio"}
      />
      <div className="flex min-w-0 flex-1 flex-col py-1 pr-2">
        <h2
          data-selectable
          className="line-clamp-2 text-[15px] leading-snug font-semibold tracking-tight text-pretty"
        >
          {file ? file.name : video.title}
        </h2>
        <p className="mt-1 truncate text-[13px] text-muted-foreground" title={file?.folder}>
          {byline}
        </p>
        {/* With its tracks open, Trim goes away: the row keeps its height, so the chips don't move. */}
        <div className={cn("mt-auto flex flex-wrap items-center gap-1.5 pt-3", tracks && "min-h-11")}>
          {video.live && <LiveBadges live={video.live} />}
          {file ? (
            <FileBadges format={video.formats[0]} quality={quality} />
          ) : (
            quality && <Badge accent>Up to {quality}</Badge>
          )}
          {best?.dynamic_range && best.dynamic_range !== "SDR" && <Badge>{best.dynamic_range}</Badge>}
          {tracks && <TracksChipButton {...tracks} />}
          {video.view_count != null && !video.live && <Badge>{formatCount(video.view_count)} views</Badge>}
          {video.with_account && (
            <Hint
              label={`Found with your ${displayHost(video.url)} sign-in, because this video needs one. It downloads with it too.`}
              className="max-w-64 text-pretty"
            >
              <span tabIndex={0} className="rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40">
                <Badge>
                  <LogIn className="mr-1 inline size-3 align-[-2px]" />
                  Signed in
                </Badge>
              </span>
            </Hint>
          )}
          {action && <div className="ml-auto">{action}</div>}
        </div>
      </div>
    </article>
  );
}

/** A file's own picture and sound: "1080p60", "H.264 + AAC". */
function FileBadges({ format, quality }: { format: Format | undefined; quality: string | null }) {
  const held = codecs(format);
  return (
    <>
      {quality && <Badge accent>{quality}</Badge>}
      {held && <Badge>{held}</Badge>}
    </>
  );
}

/** "6 tracks ›": a chip like the others that can be pressed; open, it shows ✕ to close them. */
function TracksChipButton({ count, open, onToggle, disabled }: TracksChip) {
  return (
    <Hint label={open ? "Back to converting" : "Choose which tracks it keeps, without converting"}>
      <button
        type="button"
        aria-pressed={open}
        disabled={disabled}
        onClick={onToggle}
        className={cn(
          "inline-flex items-center gap-0.5 rounded-md py-0.5 pr-1 pl-1.5 text-xs font-medium tabular-nums transition-colors outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:pointer-events-none disabled:opacity-50",
          open
            ? "bg-primary text-primary-foreground hover:bg-primary-hover"
            : "bg-primary-soft text-primary hover:bg-primary/15",
        )}
      >
        {count} tracks
        {open ? <X className="size-3" aria-hidden /> : <ChevronRight className="size-3" aria-hidden />}
      </button>
    </Hint>
  );
}

/** How long it has been on air and how many watch; when it starts; or that the channel is offline. */
function LiveBadges({ live }: { live: Live }) {
  const now = useNow();
  if (live.status === "offline") return <Badge>Not live right now</Badge>;
  if (live.status === "upcoming") {
    if (live.since == null) return <Badge>Scheduled</Badge>;
    const left = live.since - now;
    return <Badge>{left > 60 ? `Starts in ${formatSpan(left)}` : "Starting soon"}</Badge>;
  }
  return (
    <>
      {live.since != null && now > live.since && <Badge live>Live for {formatSpan(now - live.since)}</Badge>}
      {live.viewers != null && <Badge>{formatCount(live.viewers)} watching</Badge>}
    </>
  );
}

function Thumbnail({
  src,
  duration,
  live,
  sound = false,
}: {
  src: string | null;
  duration: number | null;
  live: Live | null;
  /** A sound file: without a cover, it shows that rather than a missing picture. */
  sound?: boolean;
}) {
  const [state, setState] = useState<"loading" | "loaded" | "failed">(src ? "loading" : "failed");

  return (
    <div className="relative aspect-video w-52 shrink-0 overflow-hidden rounded-xl bg-muted">
      {state === "failed" ? (
        <div className="grid size-full place-items-center text-subtle-foreground">
          {sound ? <AudioLines className="size-6" /> : <ImageOff className="size-5" />}
        </div>
      ) : (
        <>
          {state === "loading" && <div className="skeleton absolute inset-0 rounded-none" />}
          {/* Fades in and settles from a slight zoom once the image has loaded. */}
          <motion.img
            src={src ?? undefined}
            alt=""
            draggable={false}
            referrerPolicy="no-referrer"
            onLoad={() => setState("loaded")}
            onError={() => setState("failed")}
            initial={false}
            animate={state === "loaded" ? { opacity: 1, scale: 1 } : { opacity: 0, scale: 1.06 }}
            transition={{ duration: 0.6, ease: [0.16, 1, 0.3, 1] }}
            // The hover zoom is a CSS `scale`, separate from the transform Motion animates.
            className="size-full object-cover transition-[scale] duration-500 ease-out-expo group-hover/card:scale-[1.04]"
          />
        </>
      )}
      {duration != null && !live && (
        <span className="absolute right-1.5 bottom-1.5 rounded-md bg-black/70 px-1.5 py-0.5 text-[11px] leading-none font-medium text-white tabular-nums backdrop-blur-sm">
          {formatDuration(duration)}
        </span>
      )}
      {live && (
        <span
          className={cn(
            "absolute top-1.5 left-1.5 flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] leading-none font-semibold tracking-wide text-white",
            live.status === "live" ? "bg-live" : "bg-black/70 backdrop-blur-sm",
          )}
        >
          {live.status === "live" && <span aria-hidden className="size-1.5 rounded-full bg-white" />}
          {live.status === "live" ? "LIVE" : live.status === "upcoming" ? "UPCOMING" : "OFFLINE"}
        </span>
      )}
    </div>
  );
}

function Badge({ accent, live, children }: { accent?: boolean; live?: boolean; children: ReactNode }) {
  return (
    <span
      className={cn(
        "rounded-md px-1.5 py-0.5 text-xs font-medium tabular-nums",
        live ? "bg-live-soft text-live" : accent ? "bg-primary-soft text-primary" : "bg-secondary text-muted-foreground",
      )}
    >
      {children}
    </span>
  );
}

/** Placeholder with the same footprint as the card, shown while metadata loads. */
export function VideoCardSkeleton() {
  return (
    <div aria-hidden className="flex gap-4 rounded-[1.25rem] border bg-card p-3 shadow-card">
      <div className="skeleton aspect-video w-52 shrink-0 rounded-xl" />
      <div className="flex flex-1 flex-col py-1 pr-2">
        <div className="skeleton h-4 w-11/12" />
        <div className="skeleton mt-2 h-4 w-3/5" />
        <div className="skeleton mt-3 h-3 w-2/5" />
        <div className="mt-auto flex gap-1.5 pt-3">
          <div className="skeleton h-5 w-20" />
          <div className="skeleton h-5 w-16" />
        </div>
      </div>
    </div>
  );
}
