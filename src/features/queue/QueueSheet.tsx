import { CircleAlert, FolderOpen, ImageOff, Inbox, Pause, Play, RotateCw, Trash2, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Sheet } from "@/components/ui/sheet";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatEta, formatSpeed } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { isIndeterminate, STAGE_LABELS } from "@/lib/stages";
import { formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { type Download, useDownloads } from "@/stores/downloads";
import { useUi } from "@/stores/ui";

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
  const { pause, resume, retry, remove } = useDownloads.getState();
  const { status, progress } = download;
  const active = status === "queued" || status === "running";

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

      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-medium">{download.title}</p>
        <p
          className={cn(
            "text-xs tabular-nums",
            // An error needs room to be read; everything else stays on one line.
            status === "failed" ? "line-clamp-3 text-destructive" : "truncate text-muted-foreground",
          )}
        >
          {statusLine(download)}
        </p>
        {/* Hairline progress; it parks where it stopped when paused. */}
        {(active || status === "paused") && (
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

      <div className="flex shrink-0 items-start">
        {active && (
          <Action label="Pause" onClick={() => pause(download.id)}>
            <Pause />
          </Action>
        )}
        {status === "paused" && (
          <Action label="Resume" onClick={() => resume(download.id)}>
            <Play />
          </Action>
        )}
        {status === "failed" && (
          <Action label="Try again" onClick={() => retry(download.id)}>
            <RotateCw />
          </Action>
        )}
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
        <Action
          label={active ? "Cancel" : status === "paused" ? "Cancel and delete what was downloaded" : "Remove from the list"}
          onClick={() => remove(download.id)}
        >
          {active || status === "paused" ? <X /> : <Trash2 />}
        </Action>
      </div>
    </div>
  );
}

function statusLine(download: Download): ReactNode {
  const { status, progress, trim } = download;
  const clip = trim ? `${formatTimecode(trim.start)} – ${formatTimecode(trim.end)}` : null;
  switch (status) {
    case "finished":
      return ["Done", download.size != null && formatBytes(download.size), clip].filter(Boolean).join(" · ");
    case "failed":
      return (
        <>
          <CircleAlert className="mr-1 inline size-3 align-[-1.5px]" />
          {download.error?.message ?? "Failed"}
        </>
      );
    case "paused":
      return ["Paused", `${Math.floor(progress.fraction * 100)}%`, clip].filter(Boolean).join(" · ");
    default:
      return [
        STAGE_LABELS[progress.stage],
        !isIndeterminate(progress.stage) && `${Math.floor(progress.fraction * 100)}%`,
        formatSpeed(progress.speed),
        formatEta(progress.eta),
        clip,
      ]
        .filter(Boolean)
        .join(" · ");
  }
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
