import { ArrowDownToLine, FolderOpen, Play, RotateCw, X } from "lucide-react";
import {
  AnimatePresence,
  motion,
  useMotionValue,
  useMotionValueEvent,
  useSpring,
  useTransform,
} from "motion/react";
import { type ReactNode, useEffect, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatEta, formatSpeed } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { riseIn, spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { type Download, useDownloads } from "@/stores/downloads";
import type { Stage, VideoInfo } from "@/types";
import { CompletionMark } from "./CompletionMark";

const STAGE_LABELS: Record<Stage, string> = {
  preparing: "Preparing",
  downloading_video: "Downloading video",
  downloading_audio: "Downloading audio",
  downloading: "Downloading",
  merging: "Merging",
  converting: "Converting",
  finishing: "Finishing up",
};

/**
 * The big button under the video card. It morphs in place:
 * Download → live progress → "Downloaded" with open/reveal actions.
 */
export function DownloadPanel({ video }: { video: VideoInfo }) {
  const [id, setId] = useState<string | null>(null);
  const download = useDownloads((state) => (id ? state.byId[id] : undefined));
  const start = useDownloads((state) => state.start);
  const cancel = useDownloads((state) => state.cancel);
  // The bar is allowed to glide to 100% before the finished state takes over.
  const [filled, setFilled] = useState(false);

  const begin = () => {
    setFilled(false);
    setId(start(video));
  };

  const view =
    !download || download.status === "failed"
      ? "idle"
      : download.status === "finished" && filled
        ? "finished"
        : "running";

  return (
    <div className="space-y-3">
      <div
        className={cn(
          "relative h-13 overflow-hidden rounded-2xl transition-[background-color,box-shadow] duration-300",
          view === "idle" && "bg-primary shadow-[0_8px_24px_-10px_var(--primary)]",
          view === "running" && "bg-secondary",
          view === "finished" && "bg-card shadow-card ring-1 ring-border",
        )}
      >
        <AnimatePresence initial={false}>
          {view === "idle" && (
            <Face key="idle">
              <button
                type="button"
                onClick={begin}
                className="group flex size-full items-center justify-center gap-2.5 text-[15px] font-semibold text-primary-foreground transition-[background-color,transform] duration-150 hover:bg-primary-hover active:scale-[0.985]"
              >
                {download ? (
                  <RotateCw className="size-4.5 transition-transform duration-300 group-hover:rotate-45" />
                ) : (
                  <ArrowDownToLine className="size-4.5 transition-transform duration-200 group-hover:translate-y-0.5" />
                )}
                {download ? "Try again" : "Download"}
              </button>
            </Face>
          )}
          {view === "running" && download && (
            <Face key="running">
              <Running download={download} onCancel={() => cancel(download.id)} onFilled={() => setFilled(true)} />
            </Face>
          )}
          {view === "finished" && download?.path && (
            <Face key="finished">
              <Finished path={download.path} size={download.size} />
            </Face>
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence>
        {download?.status === "failed" && download.error && (
          <motion.div variants={riseIn} initial="hidden" animate="visible" exit="exit">
            <ErrorCard error={download.error} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** One state of the morphing button; faces cross-slide as the state changes. */
function Face({ children }: { children: ReactNode }) {
  return (
    <motion.div
      className="absolute inset-0"
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -16 }}
      transition={spring}
    >
      {children}
    </motion.div>
  );
}

function Running({
  download,
  onCancel,
  onFilled,
}: {
  download: Download;
  onCancel: () => void;
  onFilled: () => void;
}) {
  const { stage, speed, eta } = download.progress;
  const done = download.status === "finished";
  const fraction = done ? 1 : download.progress.fraction;

  // Progress arrives in steps; a critically damped spring turns it into a glide.
  const target = useMotionValue(fraction);
  const smooth = useSpring(target, { stiffness: 70, damping: 22, mass: 1 });
  const percent = useTransform(smooth, (value) => `${Math.floor(Math.min(value, 1) * 100)}%`);
  useEffect(() => target.set(fraction), [target, fraction]);

  useMotionValueEvent(smooth, "change", (value) => {
    if (done && value >= 0.995) onFilled();
  });
  useEffect(() => {
    if (done && smooth.get() >= 0.995) onFilled();
  }, [done, smooth, onFilled]);

  const waiting = stage === "preparing";
  const detail = [formatSpeed(speed), formatEta(eta)].filter(Boolean).join(" · ");

  return (
    <div className="relative flex size-full items-center gap-3 pr-2 pl-5">
      <motion.div className="absolute inset-0 origin-left bg-primary/15" style={{ scaleX: smooth }} />
      <motion.div className="absolute inset-x-0 bottom-0 h-0.5 origin-left bg-primary" style={{ scaleX: smooth }} />
      {waiting && (
        <div className="absolute inset-0 animate-shimmer bg-[linear-gradient(100deg,transparent_35%,var(--primary-soft)_50%,transparent_65%)] bg-size-[200%_100%] motion-reduce:animate-none" />
      )}

      {!waiting && (
        <motion.span className="relative w-11 text-[15px] font-semibold tabular-nums">{percent}</motion.span>
      )}
      <div className="relative h-5 min-w-0 flex-1 overflow-hidden text-[15px] leading-5 font-medium">
        <AnimatePresence initial={false}>
          <motion.span
            key={stage}
            className="absolute inset-0 truncate"
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -10 }}
            transition={spring}
          >
            {STAGE_LABELS[stage]}
            {waiting && "…"}
          </motion.span>
        </AnimatePresence>
      </div>
      <span className="relative shrink-0 text-[13px] text-muted-foreground tabular-nums">{detail}</span>
      <Hint label="Cancel">
        <button
          type="button"
          aria-label="Cancel download"
          onClick={onCancel}
          className="relative grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <X className="size-4" />
        </button>
      </Hint>
    </div>
  );
}

function Finished({ path, size }: { path: string; size: number | null }) {
  return (
    <div className="flex size-full items-center gap-3 pr-2 pl-4">
      <CompletionMark />
      <div className="min-w-0 flex-1 text-[15px] font-medium">
        Downloaded
        {size != null && <span className="ml-2 text-[13px] font-normal text-muted-foreground">{formatBytes(size)}</span>}
      </div>
      <Button variant="ghost" size="sm" onClick={() => void ipc.revealFile(path)}>
        <FolderOpen /> Show in folder
      </Button>
      <Button size="sm" onClick={() => void ipc.openFile(path)}>
        <Play className="fill-current" /> Open
      </Button>
    </div>
  );
}
