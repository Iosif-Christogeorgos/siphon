import { ArrowDownToLine, FolderOpen, Pause, Play, RotateCw, X } from "lucide-react";
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
import { isIndeterminate, STAGE_LABELS } from "@/lib/stages";
import { cn } from "@/lib/utils";
import { type DownloadGroup, useDownloads } from "@/stores/downloads";
import { CompletionMark } from "./CompletionMark";

/**
 * The big button under the video card. It morphs in place:
 * Download → live progress (or paused) → "Downloaded" with open/reveal actions.
 */
export function DownloadPanel({
  download,
  onStart,
  disabled,
  label,
}: {
  /** The download (or group of clips) started from this panel, if any. */
  download: DownloadGroup | undefined;
  onStart: () => void;
  /** True while there is no valid plan to download. */
  disabled: boolean;
  /** Button text: "Download", "Download clip", … */
  label: string;
}) {
  const store = useDownloads.getState();
  // The bar is allowed to glide to 100% before the finished state takes over.
  const [filled, setFilled] = useState(false);
  const downloadId = download?.id;
  useEffect(() => setFilled(false), [downloadId]);

  const view =
    !download || download.status === "failed"
      ? "idle"
      : download.status === "paused"
        ? "paused"
        : download.status === "finished" && filled
          ? "finished"
          : "running";

  return (
    <div className="space-y-3">
      <div
        className={cn(
          "relative h-13 overflow-hidden rounded-2xl transition-[background-color,box-shadow] duration-300",
          view === "idle" && "bg-primary shadow-[0_8px_24px_-10px_var(--primary)]",
          (view === "running" || view === "paused") && "bg-secondary",
          view === "finished" && "bg-card shadow-card ring-1 ring-border",
        )}
      >
        <AnimatePresence initial={false}>
          {view === "idle" && (
            <Face key="idle">
              <button
                type="button"
                onClick={download ? () => download.ids.forEach((id) => store.retry(id)) : onStart}
                disabled={disabled}
                className="group relative flex size-full items-center justify-center gap-2.5 overflow-hidden text-[15px] font-semibold text-primary-foreground transition-[background-color,transform] duration-150 hover:bg-primary-hover active:scale-[0.985] disabled:pointer-events-none disabled:opacity-60"
              >
                {download ? (
                  <RotateCw className="size-4.5 transition-transform duration-300 group-hover:rotate-45" />
                ) : (
                  <ArrowDownToLine className="size-4.5 transition-transform duration-200 group-hover:translate-y-0.5" />
                )}
                {download ? "Try again" : label}
                {/* A band of light that sweeps across once when the pointer arrives. */}
                <span
                  aria-hidden
                  className="pointer-events-none absolute inset-y-0 -left-1/3 w-1/3 -skew-x-12 bg-linear-to-r from-transparent via-white/25 to-transparent opacity-0 group-hover:left-full group-hover:opacity-100 group-hover:transition-[left] group-hover:duration-700 group-hover:ease-out motion-reduce:hidden"
                />
              </button>
            </Face>
          )}
          {view === "running" && download && (
            <Face key="running">
              <Running
                download={download}
                onPause={() => download.ids.forEach(store.pause)}
                onCancel={() => download.ids.forEach(store.remove)}
                onFilled={() => setFilled(true)}
              />
            </Face>
          )}
          {view === "paused" && download && (
            <Face key="paused">
              <Paused
                fraction={download.progress.fraction}
                onResume={() => download.ids.forEach(store.resume)}
                onCancel={() => download.ids.forEach(store.remove)}
              />
            </Face>
          )}
          {view === "finished" && download?.path && (
            <Face key="finished">
              <Finished path={download.path} size={download.size} count={download.count} />
            </Face>
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence>
        {download?.status === "failed" && download.error && (
          <motion.div variants={riseIn} initial="hidden" animate="visible" exit="exit">
            {/* Signing in retries the download by itself (see `stores/signIn`). */}
            <ErrorCard
              error={download.error}
              signIn={{ url: download.url, onUseAccount: () => download.ids.forEach((id) => store.retry(id, true)) }}
            />
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

const iconButton =
  "relative grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:bg-accent hover:text-foreground";

function Running({
  download,
  onPause,
  onCancel,
  onFilled,
}: {
  download: DownloadGroup;
  onPause: () => void;
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

  // No percentage until there is something to measure (a resumed download keeps its own).
  const waiting = isIndeterminate(stage) && fraction === 0;
  const detail = [formatSpeed(speed), formatEta(eta)].filter(Boolean).join(" · ");

  return (
    <div className="relative flex size-full items-center gap-3 pr-2 pl-5">
      <motion.div className="absolute inset-0 origin-left bg-primary/15" style={{ scaleX: smooth }} />
      <motion.div className="absolute inset-x-0 bottom-0 h-0.5 origin-left bg-primary" style={{ scaleX: smooth }} />
      {stage === "preparing" && (
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
            {stage === "preparing" && "…"}
          </motion.span>
        </AnimatePresence>
      </div>
      <span className="relative shrink-0 text-[13px] text-muted-foreground tabular-nums">{detail}</span>
      {!done && (
        <div className="relative flex">
          <Hint label="Pause">
            <button type="button" aria-label="Pause download" onClick={onPause} className={iconButton}>
              <Pause className="size-4" />
            </button>
          </Hint>
          <Hint label="Cancel">
            <button type="button" aria-label="Cancel download" onClick={onCancel} className={iconButton}>
              <X className="size-4" />
            </button>
          </Hint>
        </div>
      )}
    </div>
  );
}

function Paused({
  fraction,
  onResume,
  onCancel,
}: {
  fraction: number;
  onResume: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="relative flex size-full items-center gap-3 pr-2 pl-5">
      <div className="absolute inset-0 origin-left bg-foreground/5" style={{ transform: `scaleX(${fraction})` }} />
      <span className="relative w-11 text-[15px] font-semibold text-muted-foreground tabular-nums">
        {Math.floor(fraction * 100)}%
      </span>
      <span className="relative flex-1 text-[15px] font-medium">Paused</span>
      <Button size="sm" className="relative" onClick={onResume}>
        <Play className="fill-current" /> Resume
      </Button>
      <Hint label="Cancel and delete what was downloaded">
        <button type="button" aria-label="Cancel download" onClick={onCancel} className={iconButton}>
          <X className="size-4" />
        </button>
      </Hint>
    </div>
  );
}

function Finished({ path, size, count }: { path: string; size: number | null; count: number }) {
  return (
    <div className="flex size-full items-center gap-3 pr-2 pl-4">
      <CompletionMark />
      <div className="min-w-0 flex-1 text-[15px] font-medium">
        {count > 1 ? `${count} clips downloaded` : "Downloaded"}
        {size != null && <span className="ml-2 text-[13px] font-normal text-muted-foreground">{formatBytes(size)}</span>}
      </div>
      <Button variant="ghost" size="sm" onClick={() => void ipc.revealFile(path)}>
        <FolderOpen /> Show in folder
      </Button>
      {count === 1 && (
        <Button size="sm" onClick={() => void ipc.openFile(path)}>
          <Play className="fill-current" /> Open
        </Button>
      )}
    </div>
  );
}
