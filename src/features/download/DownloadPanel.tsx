import { ArrowDownToLine, Check, FolderOpen, Info, Pause, Play, RefreshCw, RotateCw, Square, X } from "lucide-react";
import {
  AnimatePresence,
  motion,
  useMotionValue,
  useMotionValueEvent,
  useSpring,
  useTransform,
} from "motion/react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatDuration, formatEta, formatSpan, formatSpeed, formatStreamSpeed } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { riseIn, spring } from "@/lib/motion";
import { isBusy, isLive, STAGE_LABELS } from "@/lib/stages";
import { useNow } from "@/lib/time";
import { cn } from "@/lib/utils";
import { type DownloadGroup, useDownloads } from "@/stores/downloads";
import { CompletionMark } from "./CompletionMark";

/**
 * The big button under the video card. It morphs in place:
 * Download → live progress (or paused) → "Downloaded" with open/reveal actions.
 * For a live stream: Record → REC with the time and size so far, and Stop
 * (or waiting for the stream to start) → finishing → "Recorded".
 * For a file on this PC: Convert (or what it makes) → progress → "Converted".
 */
export function DownloadPanel({
  download,
  onStart,
  disabled,
  label,
  record = false,
  convert = false,
  nothing = false,
}: {
  /** The download (or group of clips) started from this panel, if any. */
  download: DownloadGroup | undefined;
  onStart: () => void;
  /** True while there is no valid plan to download. */
  disabled: boolean;
  /** Button text: "Download", "Download clip", … */
  label: string;
  /** A live stream: the button records it. */
  record?: boolean;
  /** A file on this PC: the button converts it, and nothing is downloaded. */
  convert?: boolean;
  /** The file already is what was asked for: the button rests, saying so. */
  nothing?: boolean;
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
          : download.record && isLive(download.progress.stage)
            ? "recording"
            : "running";

  return (
    <div className="space-y-3">
      <div
        className={cn(
          "relative h-13 overflow-hidden rounded-2xl transition-[background-color,box-shadow] duration-300",
          view === "idle" && !nothing && "bg-primary shadow-[0_8px_24px_-10px_var(--primary)]",
          view === "idle" && nothing && "bg-secondary",
          (view === "running" || view === "paused" || view === "recording") && "bg-secondary",
          view === "finished" && "bg-card shadow-card ring-1 ring-border",
        )}
      >
        <AnimatePresence initial={false}>
          {view === "idle" && nothing && (
            <Face key="nothing">
              <div className="flex size-full items-center justify-center gap-2.5 text-[15px] font-medium text-muted-foreground">
                <Check className="size-4.5 text-primary" />
                {label}
              </div>
            </Face>
          )}
          {view === "idle" && !nothing && (
            <Face key="idle">
              <button
                type="button"
                onClick={download ? () => download.ids.forEach((id) => store.retry(id)) : onStart}
                disabled={disabled}
                className="group relative flex size-full items-center justify-center gap-2.5 overflow-hidden text-[15px] font-semibold text-primary-foreground transition-[background-color,transform] duration-150 hover:bg-primary-hover active:scale-[0.985] disabled:pointer-events-none disabled:opacity-60"
              >
                {download ? (
                  <RotateCw className="size-4.5 transition-transform duration-300 group-hover:rotate-45" />
                ) : convert ? (
                  <RefreshCw className="size-4.5 transition-transform duration-500 ease-out-expo group-hover:rotate-180" />
                ) : record ? (
                  // The one touch of red: the button goes on air with the stream.
                  <span
                    aria-hidden
                    className="size-3 rounded-full bg-live ring-2 ring-primary-foreground/80 transition-transform duration-200 group-hover:scale-110"
                  />
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
                // A recording being finished can't be paused, and cancelling it would throw it away.
                onPause={download.record ? undefined : () => download.ids.forEach(store.pause)}
                onCancel={download.record ? undefined : () => download.ids.forEach(store.remove)}
                onFilled={() => setFilled(true)}
              />
            </Face>
          )}
          {view === "recording" && download && (
            <Face key={`recording-${download.progress.stage === "waiting_live"}`}>
              <Recording
                download={download}
                onStop={() => download.ids.forEach(store.stop)}
                onCancel={() => download.ids.forEach(store.remove)}
              />
            </Face>
          )}
          {view === "paused" && download && (
            <Face key="paused">
              <Paused
                fraction={download.progress.fraction}
                label={download.record ? "Stopped waiting for the stream" : undefined}
                resume={download.record ? "Wait again" : undefined}
                cancel={convert ? "Cancel" : undefined}
                onResume={() => download.ids.forEach(store.resume)}
                onCancel={() => download.ids.forEach(store.remove)}
              />
            </Face>
          )}
          {view === "finished" && download?.path && (
            <Face key="finished">
              <Finished
                path={download.path}
                size={download.size}
                count={download.count}
                done={
                  download.record
                    ? "Recorded"
                    : convert
                      ? download.count > 1
                        ? `${download.count} clips cut`
                        : "Converted"
                      : undefined
                }
              />
            </Face>
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence>
        {view === "finished" && download?.note && (
          <motion.p
            variants={riseIn}
            initial="hidden"
            animate="visible"
            exit="exit"
            className="flex items-start gap-2 rounded-xl bg-secondary px-3 py-2 text-[13px] text-pretty text-muted-foreground"
          >
            <Info className="mt-0.5 size-3.5 shrink-0" />
            {download.note}
          </motion.p>
        )}
      </AnimatePresence>

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
  onPause?: () => void;
  onCancel?: () => void;
  onFilled: () => void;
}) {
  const { stage, speed, eta, heading_for } = download.progress;
  const done = download.status === "finished";
  const fraction = done ? 1 : download.progress.fraction;

  // Progress arrives in steps; a critically damped spring turns it into a glide.
  const target = useMotionValue(fraction);
  const smooth = useSpring(target, { stiffness: 70, damping: 22, mass: 1 });
  const percent = useTransform(smooth, (value) => `${Math.floor(Math.min(value, 1) * 100)}%`);
  const width = useTransform(smooth, (value) => `${Math.min(Math.max(value, 0), 1) * 100}%`);
  // The front edge comes in once there is something to lead, and goes as the bar fills.
  const edge = useTransform(smooth, [0, 0.02, 0.97, 1], [0, 1, 1, 0]);
  // A stage with a count of its own starts the bar over at once, rather than
  // sliding it back: a recording's chat, then the drawing of it.
  const shownStage = useRef(stage);
  useEffect(() => {
    if (stage !== shownStage.current && fraction < smooth.get()) smooth.jump(fraction);
    shownStage.current = stage;
    target.set(fraction);
  }, [stage, fraction, smooth, target]);

  useMotionValueEvent(smooth, "change", (value) => {
    if (done && value >= 0.995) onFilled();
  });
  useEffect(() => {
    if (done && smooth.get() >= 0.995) onFilled();
  }, [done, smooth, onFilled]);

  // No percentage until there is something to measure (a resumed download keeps
  // its own); meanwhile, a stage at work has its bar moving, never a still 0%.
  const waiting = fraction <= 0;
  const busy = isBusy(stage, fraction);
  // While converting: the size the file is heading for, from what it has written so far.
  const heading = heading_for != null ? `≈ ${formatBytes(heading_for)}` : null;
  const detail = [heading, formatSpeed(speed), formatEta(eta)].filter(Boolean).join(" · ");

  return (
    <div className="relative flex size-full items-center gap-3 pr-2 pl-5">
      {/* The fill: deepening toward its front, with light drifting through it. Sized
          rather than stretched, so its gradient and edge keep their shape. */}
      <motion.div aria-hidden className="absolute inset-y-0 left-0" style={{ width }}>
        <div className="absolute inset-0 bg-linear-to-r from-primary/5 to-primary/22" />
        <div className="absolute inset-0 animate-sheen bg-[linear-gradient(100deg,transparent_30%,color-mix(in_oklab,var(--primary)_14%,transparent)_50%,transparent_70%)] bg-size-[200%_100%] motion-reduce:hidden" />
        {/* The front: a bright edge and the light it casts back into the fill, breathing
            together. Not a glow around it: a fixed one read as a lamp following the bar. */}
        <motion.div className="absolute inset-y-0 right-0 w-12" style={{ opacity: edge }}>
          <div className="absolute inset-0 animate-breathe motion-reduce:animate-none">
            <div className="absolute inset-0 bg-linear-to-r from-transparent to-primary/25" />
            <div className="absolute inset-y-0 right-0 w-0.5 bg-linear-to-b from-transparent via-primary to-transparent" />
          </div>
        </motion.div>
      </motion.div>
      {busy && (
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
      {!done && (onPause || onCancel) && (
        <div className="relative flex">
          {onPause && (
            <Hint label="Pause">
              <button type="button" aria-label="Pause download" onClick={onPause} className={iconButton}>
                <Pause className="size-4" />
              </button>
            </Hint>
          )}
          {onCancel && (
            <Hint label="Cancel">
              <button type="button" aria-label="Cancel download" onClick={onCancel} className={iconButton}>
                <X className="size-4" />
              </button>
            </Hint>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * A recording on air: ● REC, how long and how much so far, and Stop. Before
 * the stream starts: that it's waiting, and a way to stop waiting. There is
 * no Pause: a live stream doesn't wait, so a pause would only lose that part.
 */
function Recording({
  download,
  onStop,
  onCancel,
}: {
  download: DownloadGroup;
  onStop: () => void;
  onCancel: () => void;
}) {
  const now = useNow(10_000);
  const { stage, live } = download.progress;
  const waiting = stage === "waiting_live";
  const startsIn = live?.starts_at != null ? live.starts_at - now : null;
  const status = waiting
    ? startsIn != null && startsIn > 60
      ? `Waiting for the stream · starts in ${formatSpan(startsIn)}`
      : "Waiting for the stream to start"
    : stage === "reconnecting"
      ? "Stream paused, waiting for it"
      : live?.stop_in != null
        ? `Catching up to Stop · ${formatSpan(live.stop_in)}`
        : live?.behind != null
          ? `Catching up · ${formatSpan(live.behind)} behind live`
          : null;
  const detail = waiting
    ? ""
    : [live && formatBytes(live.bytes), stage === "recording" && formatStreamSpeed(live?.bitrate)].filter(Boolean).join(" · ");

  return (
    <div className="relative flex size-full items-center gap-3 pr-2 pl-5">
      {waiting ? (
        <span aria-hidden className="size-2.5 shrink-0 rounded-full border-2 border-live" />
      ) : (
        <span className="flex shrink-0 items-center gap-1.5 text-[13px] font-semibold tracking-wide text-live">
          <span aria-hidden className="size-2.5 animate-rec rounded-full bg-live motion-reduce:animate-none" />
          REC
        </span>
      )}
      <div className="relative h-5 min-w-0 flex-1 overflow-hidden text-[15px] leading-5 font-medium">
        <AnimatePresence initial={false}>
          <motion.span
            key={status ?? "elapsed"}
            className="absolute inset-0 truncate tabular-nums"
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -10 }}
            transition={spring}
          >
            {status ?? formatDuration(live?.elapsed ?? 0)}
          </motion.span>
        </AnimatePresence>
      </div>
      <span className="relative shrink-0 text-[13px] text-muted-foreground tabular-nums">{detail}</span>
      {waiting ? (
        <Hint label="Stop waiting">
          <button type="button" aria-label="Stop waiting for the stream" onClick={onCancel} className={iconButton}>
            <X className="size-4" />
          </button>
        </Hint>
      ) : (
        <Button size="sm" variant="secondary" className="relative bg-card" onClick={onStop}>
          <Square className="fill-current" /> Stop
        </Button>
      )}
    </div>
  );
}

function Paused({
  fraction,
  label,
  resume,
  cancel,
  onResume,
  onCancel,
}: {
  fraction: number;
  /** In place of "Paused" (a recording that stopped waiting for its stream). */
  label?: string;
  resume?: string;
  /** What cancelling does, in place of deleting what was downloaded. */
  cancel?: string;
  onResume: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="relative flex size-full items-center gap-3 pr-2 pl-5">
      {/* The running fill, at rest: grey, and no light. */}
      <div aria-hidden className="absolute inset-y-0 left-0" style={{ width: `${Math.min(fraction, 1) * 100}%` }}>
        <div className="absolute inset-0 bg-linear-to-r from-foreground/3 to-foreground/8" />
        {fraction > 0 && (
          <div className="absolute inset-y-0 right-0 w-0.5 bg-linear-to-b from-transparent via-foreground/25 to-transparent" />
        )}
      </div>
      {!label && (
        <span className="relative w-11 text-[15px] font-semibold text-muted-foreground tabular-nums">
          {Math.floor(fraction * 100)}%
        </span>
      )}
      <span className="relative min-w-0 flex-1 truncate text-[15px] font-medium">{label ?? "Paused"}</span>
      <Button size="sm" className="relative" onClick={onResume}>
        <Play className="fill-current" /> {resume ?? "Resume"}
      </Button>
      <Hint label={cancel ?? "Cancel and delete what was downloaded"}>
        <button type="button" aria-label="Cancel download" onClick={onCancel} className={iconButton}>
          <X className="size-4" />
        </button>
      </Hint>
    </div>
  );
}

function Finished({ path, size, count, done }: { path: string; size: number | null; count: number; done?: string }) {
  return (
    <div className="flex size-full items-center gap-3 pr-2 pl-4">
      <CompletionMark />
      <div className="min-w-0 flex-1 text-[15px] font-medium">
        {done ?? (count > 1 ? `${count} clips downloaded` : "Downloaded")}

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
