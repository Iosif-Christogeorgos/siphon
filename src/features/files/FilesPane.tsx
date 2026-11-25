import { useQueries } from "@tanstack/react-query";
import { AudioLines, Check, Files, RefreshCw, Search, TriangleAlert, X } from "lucide-react";
import { motion } from "motion/react";
import { type MouseEvent, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { Box, fold, Thumb } from "@/features/playlist/rows";
import { PresetArea } from "@/features/presets/PresetArea";
import { formatBytes, formatDuration } from "@/lib/format";
import { ipc, toAppError } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useChoice, useChoices } from "@/stores/choices";
import { isActive, useDownloads } from "@/stores/downloads";
import { useTrim } from "@/stores/trim";
import { useUi } from "@/stores/ui";
import type { Intake, LocalFile, PlanSummary, VideoInfo } from "@/types";

/** Long enough a list that finding a file by name helps. */
const SEARCH_FROM = 10;

/** Rows rise in one after another, the first dozen or so; the rest at once. */
const STAGGER = 0.025;
const STAGGERED = 14;

/** "C:\Users\Ana\Videos" → "Videos". */
const folderName = (folder: string) => folder.split(/[\\/]/).filter(Boolean).at(-1) ?? folder;

/**
 * "This already plays on Windows 11. Nothing to convert." → "Already plays on
 * Windows 11": the reason a file is left out, short enough for its row.
 */
function shortReason(nothing: string): string {
  const first = nothing.split(". ")[0] ?? nothing;
  const clause = first.split(", ")[0] ?? first;
  return clause.replace(/^This (is )?already/, "Already").replace(/\.$/, "");
}

/** Where a file stands in the list. */
type Status =
  | { kind: "reading" }
  | { kind: "unreadable"; why: string }
  | { kind: "planning"; video: VideoInfo }
  | { kind: "nothing"; video: VideoInfo; why: string }
  | { kind: "queued"; video: VideoInfo }
  | { kind: "ready"; video: VideoInfo; plan: PlanSummary };

/**
 * Several files on this PC: pick which to convert, with one preset for all,
 * and convert them in one go. Each file is read (three at a time, by the
 * backend) and planned on its own as the list fills in; one that already is
 * what was asked for is marked and left out, and so is one that can't be
 * read. What is made goes beside each file.
 */
export function FilesPane({ intake }: { intake: Intake }) {
  const choice = useChoice();
  const start = useDownloads((state) => state.start);
  const openPanel = useUi((state) => state.openPanel);
  const files = intake.files;

  // Stream picks and trim ranges belong to a single video, not to a batch.
  useEffect(() => {
    useChoices.getState().resetForVideo();
    useTrim.getState().reset();
  }, []);

  const reads = useQueries({
    queries: files.map((file) => ({
      // A file dropped again after it changed is read again (its version is new).
      queryKey: ["file", file.key, file.version],
      queryFn: () => ipc.readFile(file.key),
      staleTime: Number.POSITIVE_INFINITY,
      retry: false,
    })),
  });
  const plans = useQueries({
    queries: files.map((file, index) => ({
      // The same plan the file's own pane asks for (see `VideoPane`).
      queryKey: ["plan", file.key, choice, null, false, file.version],
      queryFn: () => (choice ? ipc.planDownload(file.key, choice, null) : Promise.reject(new Error("no choice"))),
      enabled: choice !== null && reads[index]?.isSuccess === true,
      retry: false,
    })),
  });
  const busy = useBusyKeys(files);

  const statuses: Status[] = files.map((file, index) => {
    const read = reads[index];
    const plan = plans[index];
    if (!read || read.isPending) return { kind: "reading" };
    if (read.isError) return { kind: "unreadable", why: toAppError(read.error).message };
    const video = read.data;
    if (busy.has(file.key)) return { kind: "queued", video };
    if (!plan?.data) return { kind: "planning", video };
    if (plan.data.nothing) return { kind: "nothing", video, why: plan.data.nothing };
    return { kind: "ready", video, plan: plan.data };
  });

  // Every file starts selected; one with nothing to do, or that can't be read, can't be.
  const [selected, setSelected] = useState(() => new Set(files.map((file) => file.key)));
  const selectable = (index: number) => {
    const kind = statuses[index]?.kind;
    return kind !== "unreadable" && kind !== "nothing" && kind !== "queued";
  };
  const [query, setQuery] = useState("");
  const shown = useMemo(() => {
    const words = fold(query).split(/\s+/).filter(Boolean);
    return files
      .map((file, index) => ({ file, index }))
      .filter(({ file }) => words.every((word) => fold(file.name).includes(word)));
  }, [files, query]);

  const toggle = (keys: string[], on: boolean) =>
    setSelected((current) => {
      const next = new Set(current);
      for (const key of keys) {
        if (on) next.add(key);
        else next.delete(key);
      }
      return next;
    });

  // Shift-click ticks or unticks everything between the last row clicked and this one.
  const anchor = useRef<number | null>(null);
  const onRow = (index: number, event: MouseEvent) => {
    const file = files[index];
    if (!file) return;
    const on = !selected.has(file.key);
    const from = anchor.current === null ? -1 : shown.findIndex((row) => row.index === anchor.current);
    const to = shown.findIndex((row) => row.index === index);
    if (event.shiftKey && from >= 0 && to >= 0) {
      const range = shown.slice(Math.min(from, to), Math.max(from, to) + 1).filter((row) => selectable(row.index));
      toggle(
        range.map((row) => row.file.key),
        on,
      );
    } else {
      toggle([file.key], on);
    }
    anchor.current = index;
  };

  const chosen = files.flatMap((file, index) => (selected.has(file.key) && selectable(index) ? [index] : []));
  const shownSelectable = shown.filter((row) => selectable(row.index));
  const allShown = shownSelectable.length > 0 && shownSelectable.every((row) => selected.has(row.file.key));
  // Still being read or planned: a file is only started once its plan says there is something to make.
  const waiting = chosen.filter((index) => statuses[index]?.kind !== "ready").length;
  const total = chosen.reduce((sum, index) => {
    const status = statuses[index];
    return sum + (status?.kind === "ready" ? (status.plan.estimated_size ?? 0) : 0);
  }, 0);
  const leftOut = statuses.filter((status) => status.kind === "nothing").length;
  const unreadable = statuses.filter((status) => status.kind === "unreadable").length;

  const [added, setAdded] = useState(0);
  const convert = () => {
    if (!choice || waiting > 0) return;
    for (const index of chosen) {
      const file = files[index];
      const status = statuses[index];
      if (!file || status?.kind !== "ready") continue;
      start({ url: file.key, title: file.name, thumbnail: status.video.thumbnail }, choice, null);
    }
    setSelected(new Set());
    setAdded(chosen.length);
    openPanel("downloads");
  };

  const folders = new Set(files.map((file) => file.folder));
  const onlyFolder = folders.size === 1 ? folderName(files[0]?.folder ?? "") : null;
  const originals = files.reduce((sum, file) => sum + file.size, 0);

  return (
    <>
      <div className="flex gap-3.5 rounded-[1.25rem] border bg-card p-4 shadow-card">
        <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary-soft text-primary">
          <Files className="size-4.5" />
        </div>
        <div className="min-w-0">
          <h2 data-selectable className="truncate font-semibold tracking-tight">
            {files.length} files{onlyFolder && <span className="font-normal text-muted-foreground"> from {onlyFolder}</span>}
          </h2>
          <p className="mt-0.5 truncate text-[13px] text-muted-foreground">
            {[
              formatBytes(originals),
              leftOut > 0 && `${leftOut} with nothing to do`,
              unreadable > 0 && `${unreadable} can’t be read`,
              intake.left_out > 0 && `${intake.left_out} not video or sound, left out`,
              intake.over > 0 && `${intake.over} more left out (${files.length} at most at once)`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
      </div>

      <div className="overflow-hidden rounded-[1.25rem] border bg-card shadow-card">
        <div className="flex items-center justify-between gap-2 border-b py-2 pr-2 pl-4">
          <span className="min-w-0 truncate text-[13px] text-muted-foreground tabular-nums">
            {chosen.length} of {files.length} selected
            {chosen.length > 0 && choice && (
              <span className="text-subtle-foreground">
                {" · "}
                {waiting > 0 ? `reading ${waiting} more…` : `≈ ${formatBytes(total)}`}
              </span>
            )}
          </span>
          {shownSelectable.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              className="shrink-0"
              onClick={() =>
                toggle(
                  shownSelectable.map((row) => row.file.key),
                  !allShown,
                )
              }
            >
              {allShown ? "Select none" : query ? "Select all shown" : "Select all"}
            </Button>
          )}
        </div>
        {files.length >= SEARCH_FROM && (
          <label className="flex items-center gap-2 border-b px-4 py-1.5">
            <Search className="size-3.5 shrink-0 text-subtle-foreground" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => event.key === "Escape" && query && (event.stopPropagation(), setQuery(""))}
              placeholder="Find by name"
              aria-label="Find by name"
              spellCheck={false}
              className="h-7 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-subtle-foreground"
            />
            {query && (
              <>
                <span className="shrink-0 text-xs text-subtle-foreground tabular-nums">{shown.length} found</span>
                <button
                  type="button"
                  aria-label="Clear the search"
                  onClick={() => setQuery("")}
                  className="grid size-6 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  <X className="size-3.5" />
                </button>
              </>
            )}
          </label>
        )}
        <ul className="scrollbar-thin max-h-72 overflow-y-auto p-1.5 select-none">
          {shown.map(({ file, index }, position) => (
            <motion.li
              key={file.key}
              initial={{ opacity: 0, y: 6, filter: "blur(3px)" }}
              animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              transition={{ ...spring, delay: Math.min(position, STAGGERED) * STAGGER }}
            >
              <FileRow
                file={file}
                status={statuses[index] ?? { kind: "reading" }}
                checked={selected.has(file.key) && selectable(index)}
                onClick={(event) => onRow(index, event)}
              />
            </motion.li>
          ))}
          {shown.length === 0 && (
            <li className="px-3 py-6 text-center text-[13px] text-muted-foreground">Nothing in this list matches “{query}”.</li>
          )}
        </ul>
      </div>

      <div className="pt-1">
        <PresetArea summary={undefined} disabled={false} file />
      </div>
      <p className="px-1 text-[13px] text-pretty text-muted-foreground">
        Each file is planned from what it holds, and what is made goes beside it. Files that already are what you chose
        are left as they are.
      </p>

      <Button
        size="lg"
        className="h-13 w-full rounded-2xl"
        disabled={chosen.length === 0 || waiting > 0 || !choice}
        onClick={convert}
      >
        <RefreshCw className="size-4.5" />
        {chosen.length === 0
          ? "Select files to convert"
          : waiting > 0
            ? `Reading the files… ${chosen.length - waiting} of ${chosen.length}`
            : `Convert ${chosen.length} ${chosen.length === 1 ? "file" : "files"}`}
      </Button>
      {added > 0 && (
        <p role="status" className="text-center text-[13px] text-muted-foreground">
          Started {added} {added === 1 ? "file" : "files"}. Follow them in{" "}
          <button
            type="button"
            onClick={() => openPanel("downloads")}
            className="font-medium text-primary underline-offset-2 hover:underline"
          >
            Downloads
          </button>
          .
        </p>
      )}
    </>
  );
}

/** The files being converted or waiting to be: they can't be started twice. */
function useBusyKeys(files: LocalFile[]): Set<string> {
  const keys = useMemo(() => new Set(files.map((file) => file.key)), [files]);
  const busy = useDownloads(
    useShallow((state) =>
      state.order.flatMap((id) => {
        const download = state.byId[id];
        return download && keys.has(download.url) && (isActive(download) || download.status === "paused")
          ? [download.url]
          : [];
      }),
    ),
  );
  return useMemo(() => new Set(busy), [busy]);
}

function FileRow({
  file,
  status,
  checked,
  onClick,
}: {
  file: LocalFile;
  status: Status;
  checked: boolean;
  onClick: (event: MouseEvent) => void;
}) {
  const video = "video" in status ? status.video : null;
  const off = status.kind === "unreadable" || status.kind === "nothing" || status.kind === "queued";
  const sound = video?.formats[0]?.kind === "audio";
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-disabled={off}
      onClick={off ? undefined : onClick}
      className={cn(
        "flex w-full items-center gap-3 rounded-xl px-2 py-1.5 text-left transition-colors",
        off ? "cursor-default" : "hover:bg-accent",
      )}
    >
      <Box checked={checked} disabled={off} />
      {status.kind === "reading" ? (
        <span className="skeleton aspect-video w-14 shrink-0 rounded-md" />
      ) : (
        <Thumb src={video?.thumbnail ?? null} empty={sound ? <AudioLines className="size-3.5" /> : undefined} />
      )}
      <span className={cn("min-w-0 flex-1 truncate text-[13px]", !checked && "text-muted-foreground")}>{file.name}</span>
      <Badge status={status} />
      <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
        {[video?.duration != null && formatDuration(video.duration), formatBytes(file.size)].filter(Boolean).join(" · ")}
      </span>
    </button>
  );
}

function Badge({ status }: { status: Status }) {
  switch (status.kind) {
    case "reading":
      return <span className="shrink-0 text-[11px] text-subtle-foreground">Reading…</span>;
    case "unreadable":
      return (
        <Hint label={status.why} className="max-w-64 text-pretty">
          <span className="flex shrink-0 items-center gap-1 rounded-md bg-destructive-soft px-1.5 py-0.5 text-[11px] font-medium text-destructive">
            <TriangleAlert className="size-3" />
            Can’t be read
          </span>
        </Hint>
      );
    case "nothing":
      return (
        <Hint label={status.why} className="max-w-64 text-pretty">
          <span className="flex shrink-0 items-center gap-1 rounded-md bg-success/12 px-1.5 py-0.5 text-[11px] font-medium text-success">
            <Check className="size-3" strokeWidth={2.5} />
            {shortReason(status.why)}
          </span>
        </Hint>
      );
    case "queued":
      return (
        <span className="shrink-0 rounded-md bg-secondary px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
          In queue
        </span>
      );
    default:
      return null;
  }
}
