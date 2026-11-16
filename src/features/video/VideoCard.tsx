import { ImageOff } from "lucide-react";
import { motion } from "motion/react";
import { type ReactNode, useState } from "react";
import { formatCount, formatDuration } from "@/lib/format";
import { bestVideoFormat, resolutionLabel } from "@/lib/formats";
import { displayHost } from "@/lib/url";
import { cn } from "@/lib/utils";
import type { VideoInfo } from "@/types";

export function VideoCard({ video, action }: { video: VideoInfo; action?: ReactNode }) {
  const best = bestVideoFormat(video.formats);
  const quality = best ? resolutionLabel(best) : null;
  const byline = [video.channel, displayHost(video.url)].filter(Boolean).join(" · ");

  return (
    <article className="flex gap-4 rounded-[1.25rem] border bg-card p-3 shadow-card">
      <Thumbnail src={video.thumbnail} duration={video.duration} />
      <div className="flex min-w-0 flex-1 flex-col py-1 pr-2">
        <h2
          data-selectable
          className="line-clamp-2 text-[15px] leading-snug font-semibold tracking-tight text-pretty"
        >
          {video.title}
        </h2>
        <p className="mt-1 truncate text-[13px] text-muted-foreground">{byline}</p>
        <div className="mt-auto flex flex-wrap items-center gap-1.5 pt-3">
          {quality && <Badge accent>Up to {quality}</Badge>}
          {best?.dynamic_range && best.dynamic_range !== "SDR" && <Badge>{best.dynamic_range}</Badge>}
          {video.view_count != null && <Badge>{formatCount(video.view_count)} views</Badge>}
          {action && <div className="ml-auto">{action}</div>}
        </div>
      </div>
    </article>
  );
}

function Thumbnail({ src, duration }: { src: string | null; duration: number | null }) {
  const [state, setState] = useState<"loading" | "loaded" | "failed">(src ? "loading" : "failed");

  return (
    <div className="relative aspect-video w-52 shrink-0 overflow-hidden rounded-xl bg-muted">
      {state === "failed" ? (
        <div className="grid size-full place-items-center text-subtle-foreground">
          <ImageOff className="size-5" />
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
            className="size-full object-cover"
          />
        </>
      )}
      {duration != null && (
        <span className="absolute right-1.5 bottom-1.5 rounded-md bg-black/70 px-1.5 py-0.5 text-[11px] leading-none font-medium text-white tabular-nums backdrop-blur-sm">
          {formatDuration(duration)}
        </span>
      )}
    </div>
  );
}

function Badge({ accent, children }: { accent?: boolean; children: ReactNode }) {
  return (
    <span
      className={cn(
        "rounded-md px-1.5 py-0.5 text-xs font-medium",
        accent ? "bg-primary-soft text-primary" : "bg-secondary text-muted-foreground",
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
