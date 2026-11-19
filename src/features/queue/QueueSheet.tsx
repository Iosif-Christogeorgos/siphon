import {
  Check,
  CircleAlert,
  FolderOpen,
  HardDrive,
  ImageOff,
  Inbox,
  Info,
  LogIn,
  Pause,
  Play,
  RotateCw,
  Scissors,
  Square,
  Trash2,
  X,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useRef } from "react";
import { Button } from "@/components/ui/button";
import { Sheet } from "@/components/ui/sheet";
import { Hint } from "@/components/ui/tooltip";
import { formatBitrate, formatBytes, formatDuration, formatEta, formatSpan, formatSpeed } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { isIndeterminate, isLive, STAGE_LABELS } from "@/lib/stages";
import { formatShortTimecode, formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { type Download, useDownloads } from "@/stores/downloads";
import { signInSuggested } from "@/stores/signIn";
import { useUi } from "@/stores/ui";
import type { Stage } from "@/types";

/** Where each status sits in the list: what is happening now on top, history below. */
const STATUS_ORDER: Record<Download["status"], number> = { running: 0, queued: 1, paused: 2, failed: 3, finished: 4 };

/**
 * The download queue: everything started this session. Running downloads
 * come first, then the ones waiting (in the order they will start), then
 * paused, failed and finished ones (newest first).
 */
export function QueueSheet() {
  const open = useUi((state) => state.panel === "downloads");
  const closePanel = useUi((state) => state.closePanel);
  const order = useDownloads((state) => state.order);
  const byId = useDownloads((state) => state.byId);
  const clearFinished = useDownloads((state) => state.clearFinished);
  const resumeAll = useDownloads((state) => state.resumeAll);

  const downloads = order
    .map((id, index) => ({ download: byId[id], index }))
    .filter((entry): entry is { download: Download; index: number } => entry.download !== undefined)
    .sort((a, b) => {
      const group = STATUS_ORDER[a.download.status] - STATUS_ORDER[b.download.status];
      if (group !== 0) return group;
      // Waiting and running: first in, first out. History: newest first.
      return a.download.status === "queued" || a.download.status === "running" ? a.index - b.index : b.index - a.index;
    })
    .map((entry) => entry.download);
  const hasFinished = downloads.some((download) => download.status === "finished");
  const paused = downloads.filter((download) => download.status === "paused").length;

  return (
    <Sheet open={open} onOpenChange={(next) => !next && closePanel()} title="Downloads">
      {downloads.length === 0 ? (
        <div className="flex flex-col items-center gap-2 pt-24 text-center text-muted-foreground">
          <Inbox className="size-6 text-subtle-foreground" strokeWidth={1.5} />
          <p className="max-w-64 text-[13px] text-balance">Nothing here yet. Downloads you start show up in this list.</p>
        </div>
      ) : (
        <>
          <ul className="-mx-2">
            <AnimatePresence initial={false}>
              {downloads.map((download) => (
                <motion.li
                  key={download.id}
                  layout
                  initial={{ opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: "auto" }}
                  exit={{ opacity: 0, height: 0 }}
                  transition={spring}
                  className="overflow-hidden"
                >
                  <QueueItem download={download} />
                </motion.li>
              ))}
            </AnimatePresence>
          </ul>
          {(hasFinished || paused > 1) && (
            <div className="mt-3 flex justify-end gap-1">
              {paused > 1 && (
                <Button variant="ghost" size="sm" onClick={resumeAll}>
                  <Play /> Resume all
                </Button>
              )}
              {hasFinished && (
                <Button variant="ghost" size="sm" onClick={clearFinished}>
                  Clear finished
                </Button>
              )}
            </div>
          )}
        </>
      )}
    </Sheet>
  );
}

function QueueItem({ download }: { download: Download }) {
  const { pause, resume, remove, stop } = useDownloads.getState();
  const { status, progress } = download;
  const active = status === "queued" || status === "running";
  const text = useRef<HTMLDivElement>(null);
  // A recording has no Pause: on air it has Stop, which keeps what it recorded.
  const onAir = download.record && active && (progress.stage === "recording" || progress.stage === "reconnecting");
  const finishing = download.record && active && progress.stage === "finalizing";

  return (
    <div className="group relative flex gap-3 rounded-xl px-2 py-2.5 transition-colors hover:bg-accent">
      <div className="relative aspect-video w-16 shrink-0 self-start overflow-hidden rounded-md bg-muted">
        {download.thumbnail ? (
          <img
            src={download.thumbnail}
            alt=""
            draggable={false}
            referrerPolicy="no-referrer"
            className="size-full object-cover"
          />
        ) : (
          <div className="grid size-full place-items-center text-subtle-foreground">
            <ImageOff className="size-3.5" />
          </div>
        )}
      </div>

      <Hint
        // The whole title and status, but only when the row had to leave some of it out.
        label={
          <>
            <span className="block font-medium">{download.title}</span>
            <span className="block text-muted-foreground">{fullStatus(download)}</span>
          </>
        }
        className="max-w-72"
        side="bottom"
        when={() => isCutOff(text.current)}
      >
        <div ref={text} className="min-w-0 flex-1">
          <p className="line-clamp-2 text-[13px] leading-snug font-medium wrap-break-word">{download.title}</p>
          {status === "failed" ? (
            // An error needs room to be read.
            <p className="mt-0.5 line-clamp-3 text-xs text-destructive">
              <CircleAlert className="mr-1 inline size-3 align-[-1.5px]" />
              {download.error?.message ?? "Failed"}
            </p>
          ) : (
            // One line of pieces, most important first: a piece that doesn't fit wraps
            // onto the hidden second line, so it is left out whole instead of cut in half.
            <p className="mt-0.5 flex h-4 flex-wrap content-start items-center overflow-hidden text-xs leading-4 text-muted-foreground tabular-nums">
              {statusPieces(download).map((piece, index) => (
                <span key={piece.key} className="flex max-w-full items-center gap-1 whitespace-nowrap [&_svg]:size-3 [&_svg]:shrink-0">
                  {index > 0 && <span aria-hidden className="ml-1 text-subtle-foreground">·</span>}
                  {piece.node}
                </span>
              ))}
            </p>
          )}
          {/* Hairline progress; it parks where it stopped when paused. A recording on air has nothing to fill. */}
          {(active || status === "paused") && !(download.record && isLive(progress.stage)) && (
            <div className="mt-1.5 h-0.5 overflow-hidden rounded-full bg-border-strong">
              <motion.div
                className={cn("h-full origin-left", status === "paused" ? "bg-subtle-foreground" : "bg-primary")}
                initial={false}
                animate={{ scaleX: progress.fraction }}
                transition={{ type: "spring", stiffness: 70, damping: 22 }}
              />
            </div>
          )}
        </div>
      </Hint>

      <div className="flex shrink-0 items-start">
        {onAir && (
          <Action label="Stop and save the recording" onClick={() => stop(download.id)}>
            <Square />
          </Action>
        )}
        {active && !download.record && (
          <Action label="Pause" onClick={() => pause(download.id)}>
            <Pause />
          </Action>
        )}
        {status === "paused" && (
          <Action label="Resume" onClick={() => resume(download.id)}>
            <Play />
          </Action>
        )}
        {status === "failed" && <FailedAction download={download} />}
        {status === "finished" && download.path && (
          <>
            <Action label="Open" onClick={() => void ipc.openFile(download.path as string)}>
              <Play />
            </Action>
            <Action label="Show in folder" onClick={() => void ipc.revealFile(download.path as string)}>
              <FolderOpen />
            </Action>
          </>
        )}
        {/* Neither on air (Stop keeps it) nor being finished: cancelling either would throw a recording away. */}
        {!onAir && !finishing && (
          <Action
            label={
              active
                ? download.record
                  ? "Stop waiting"
                  : "Cancel"
                : status === "paused"
                  ? download.record
                    ? "Remove from the list"
                    : "Cancel and delete what was downloaded"
                  : "Remove from the list"
            }
            onClick={() => remove(download.id)}
          >
            {active || status === "paused" ? <X /> : <Trash2 />}
          </Action>
        )}
      </div>
    </div>
  );
}

/**
 * What a failed row offers: signing in where an account is the likely fix and
 * Siphon has none (signing in retries it by itself), the sign-in Siphon has
 * where the site's answer leaves it to the user, otherwise Try again.
 */
function FailedAction({ download }: { download: Download }) {
  const { retry } = useDownloads.getState();
  const need = download.error?.sign_in;
  const likely = need && (need.chance === "needed" || need.chance === "maybe");
  if (need && likely && (!need.signed_in || (need.refused && need.chance === "needed"))) {
    return (
      <Action label={`Sign in to ${need.site}`} onClick={() => void signInSuggested(download.url, need.site)}>
        <LogIn />
      </Action>
    );
  }
  if (need?.signed_in && !need.refused) {
    return (
      <Action label={`Try with your ${need.site} sign-in`} onClick={() => retry(download.id, true)}>
        <LogIn />
      </Action>
    );
  }
  return (
    <Action label="Try again" onClick={() => retry(download.id)}>
      <RotateCw />
    </Action>
  );
}

/** The list's short names for the stages; downloading needs none, the percentage says it. */
const SHORT_STAGES: Record<Stage, string | null> = {
  queued: "Waiting",
  updating: "Updating engine",
  retrying: "Retrying soon",
  preparing: "Preparing",
  downloading_video: null,
  downloading_audio: null,
  downloading: null,
  merging: "Merging",
  converting: "Converting",
  finishing: "Finishing",
  waiting_live: "Waiting to go live",
  recording: null,
  reconnecting: "Stream paused",
  finalizing: "Finishing",
};

const percent = (download: Download) =>
  isIndeterminate(download.progress.stage) ? null : `${Math.floor(download.progress.fraction * 100)}%`;

/** A recording's status line: REC and how long, how much, and how fast; or what it waits for. */
function recordingPieces(download: Download): ({ key: string; node: ReactNode } | "" | false | null | undefined)[] {
  const { progress } = download;
  const live = progress.live;
  if (progress.stage === "waiting_live") {
    const startsIn = live?.starts_at != null ? live.starts_at - Date.now() / 1000 : null;
    return [
      { key: "stage", node: "Waiting to go live" },
      startsIn != null && startsIn > 60 && { key: "starts", node: `starts in ${formatSpan(startsIn)}` },
    ];
  }
  return [
    {
      key: "rec",
      node:
        progress.stage === "reconnecting" ? (
          "Stream paused"
        ) : live?.behind != null ? (
          <span className="text-live">Catching up · {formatSpan(live.behind)} behind</span>
        ) : (
          <>
            <span aria-hidden className="size-1.5 animate-rec rounded-full bg-live motion-reduce:animate-none" />
            <span className="font-medium text-live">REC</span> {formatDuration(live?.elapsed ?? 0)}
          </>
        ),
    },
    live && { key: "size", node: formatBytes(live.bytes) },
    live?.disk_left != null && {
      key: "disk",
      node: (
        <span className="text-destructive">
          <HardDrive className="mr-0.5 inline align-[-1.5px]" />
          full in {formatSpan(live.disk_left)}
        </span>
      ),
    },
    progress.stage === "recording" && formatBitrate(live?.bitrate) && { key: "rate", node: formatBitrate(live?.bitrate) },
  ];
}

/** The status line's pieces, most important first: the ones at the end go first when the row is narrow. */
function statusPieces(download: Download): { key: string; node: ReactNode }[] {
  const { status, progress, trim } = download;
  const clip = trim && {
    key: "clip",
    node: (
      <>
        <Scissors />
        {formatShortTimecode(trim.start)}–{formatShortTimecode(trim.end)}
      </>
    ),
  };
  const pieces: ({ key: string; node: ReactNode } | "" | false | null | undefined)[] =
    status === "finished"
      ? [
          {
            key: "done",
            node: (
              <>
                <Check className="text-primary" />
                {download.size != null ? formatBytes(download.size) : "Done"}
              </>
            ),
          },
          download.recovered && { key: "recovered", node: "Recovered" },
          download.note && {
            key: "note",
            node: (
              <>
                <Info />
                Saved as .ts
              </>
            ),
          },
          clip,
        ]
      : status === "paused"
        ? [{ key: "paused", node: download.record ? "Stopped waiting" : `Paused ${Math.floor(progress.fraction * 100)}%` }, clip]
        : download.record && isLive(progress.stage)
          ? recordingPieces(download)
          : [
            {
              key: "stage",
              node: [SHORT_STAGES[progress.stage], percent(download)].filter(Boolean).join(" ") || "Downloading",
            },
            clip,
              formatEta(progress.eta) && { key: "eta", node: formatEta(progress.eta) },
              formatSpeed(progress.speed) && { key: "speed", node: formatSpeed(progress.speed) },
            ];
  return pieces.filter((piece): piece is { key: string; node: ReactNode } => Boolean(piece));
}

/** Everything the status line could say, in full, for the hint. */
function fullStatus(download: Download): string {
  const { status, progress, trim } = download;
  const clip = trim && `Clip ${formatTimecode(trim.start)} – ${formatTimecode(trim.end)}`;
  const live = progress.live;
  const parts =
    status === "finished"
      ? [
          download.recovered ? "Recovered recording" : "Done",
          download.size != null && formatBytes(download.size),
          clip,
          download.note,
        ]
      : status === "failed"
        ? [download.error?.message ?? "Failed"]
        : status === "paused"
          ? [download.record ? "Stopped waiting for the stream" : "Paused", !download.record && percent(download), clip]
          : download.record && isLive(progress.stage)
            ? [
                STAGE_LABELS[progress.stage],
                live && progress.stage !== "waiting_live" && formatDuration(live.elapsed),
                live && progress.stage !== "waiting_live" && formatBytes(live.bytes),
                live?.behind != null && `${formatSpan(live.behind)} behind live`,
                formatBitrate(live?.bitrate),
                live?.disk_left != null && `the disk is full in ${formatSpan(live.disk_left)}`,
              ]
            : [STAGE_LABELS[progress.stage], percent(download), formatSpeed(progress.speed), formatEta(progress.eta), clip];
  return parts.filter(Boolean).join(" · ");

}

/** Whether a row's text left anything out: a clamped title or error, or a status piece that didn't fit. */
function isCutOff(text: HTMLElement | null): boolean {
  if (!text) return false;
  return [...text.querySelectorAll<HTMLElement>("p, p > span")].some(
    (element) => element.scrollHeight > element.clientHeight + 1 || element.scrollWidth > element.clientWidth + 1,
  );
}

function Action({ label, onClick, children }: { label: string; onClick: () => void; children: ReactNode }) {
  return (
    <Hint label={label}>
      <button
        type="button"
        aria-label={label}
        onClick={onClick}
        className="grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-border-strong hover:text-foreground [&_svg]:size-3.5"
      >
        {children}
      </button>
    </Hint>
  );
}
