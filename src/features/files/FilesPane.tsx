import { useQueries } from "@tanstack/react-query";
import { AudioLines, Check, ChevronDown, Download, Files, RefreshCw, Search, TriangleAlert, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type MouseEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { ManualPanel } from "@/features/options/ManualPanel";
import { Box, fold, Thumb } from "@/features/playlist/rows";
import { PresetArea } from "@/features/presets/PresetArea";
import { changesOf, type Draft, draftOf, editOf, sizeOf } from "@/features/tracks/draft";
import { tracksQuery, useLanguagesKey } from "@/features/tracks/reading";
import { type SeasonFile, toCheck } from "@/features/tracks/season";
import { SeasonTracks, UnsureMark } from "@/features/tracks/SeasonTracks";
import { hasTracksToChoose, languagesLine, TracksDeck } from "@/features/tracks/TracksCard";
import { TracksPanel } from "@/features/tracks/TracksPanel";
import { formatBytes, formatDuration } from "@/lib/format";
import { ipc, toAppError } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useChoice, useChoices } from "@/stores/choices";
import { isActive, useDownloads } from "@/stores/downloads";
import { useTrim } from "@/stores/trim";
import { useUi } from "@/stores/ui";
import type { Intake, LocalFile, MkvContents, PlanSummary, VideoInfo } from "@/types";

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

/** Where a file stands in the list while the season's tracks are chosen. */
type TrackStatus =
  | { kind: "none" }
  | { kind: "reading" }
  | { kind: "failed"; why: string }
  | { kind: "ready"; contents: MkvContents; draft: Draft };

/**
 * Several files on this PC: pick which to convert, with one preset for all,
 * and convert them in one go. Each file is read (three at a time, by the
 * backend) and planned on its own as the list fills in; one that already is
 * what was asked for is marked and left out, and so is one that can't be
 * read. What is made goes beside each file.
 */
export function FilesPane({ intake }: { intake: Intake }) {
  const choice = useChoice();
  const customizing = useChoices((state) => state.choices !== null && state.choices.active.kind !== "preset");
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

  // A season: the MKVs among them can have their tracks chosen together, in
  // place of converting (docs/mkv-editor.md). They are read for it once asked.
  const [tracksOn, setTracksOn] = useState(false);
  const [tracksAsked, setTracksAsked] = useState(false);
  const languages = useLanguagesKey();
  const mkvs = files.flatMap((file, index) => {
    const local = reads[index]?.data?.file;
    return local && hasTracksToChoose(local.overview, local.matroska) ? [{ file, index, local }] : [];
  });
  const trackReads = useQueries({
    queries: mkvs.map(({ file }) => tracksQuery(file.key, file.version, languages, tracksAsked)),
  });
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const current = (key: string, version: string) => {
    const draft = drafts[key];
    return draft && draft.version === version && draft.languages === languages ? draft : null;
  };
  // Each file read opens as a single file does: with what the user's languages keep.
  const fresh = mkvs.flatMap(({ file }, place) => {
    const data = trackReads[place]?.data;
    return data && languages !== null && !current(file.key, file.version)
      ? [draftOf(data, file.key, file.version, languages)]
      : [];
  });
  useEffect(() => {
    if (fresh.length > 0) setDrafts((now) => ({ ...now, ...Object.fromEntries(fresh.map((draft) => [draft.key, draft])) }));
  }, [fresh]);
  const setDraft = (draft: Draft) => setDrafts((now) => ({ ...now, [draft.key]: draft }));
  const trackStatuses: TrackStatus[] = files.map((file) => {
    const place = mkvs.findIndex((mkv) => mkv.file.key === file.key);
    if (place < 0) return { kind: "none" };
    const read = trackReads[place];
    if (read?.isError) return { kind: "failed", why: toAppError(read.error).message };
    const draft = current(file.key, file.version);
    if (!read?.data || !draft) return { kind: "reading" };
    return { kind: "ready", contents: read.data, draft };
  });
  const seasonFiles: SeasonFile[] = files.flatMap((file, index) => {
    const status = trackStatuses[index];
    return status?.kind === "ready"
      ? [{ key: file.key, name: file.name, contents: status.contents, draft: status.draft }]
      : [];
  });
  const tracksReading = trackStatuses.filter((status) => status.kind === "reading").length;
  const [openFile, setOpenFile] = useState<string | null>(null);
  const openTracks = (on: boolean) => {
    setTracksAsked(true);
    setTracksOn(on);
  };

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
    if (tracksOn) {
      // A copy is made of a file whose tracks change, or that is still being read.
      const status = trackStatuses[index];
      if (busy.has(files[index]?.key ?? "")) return false;
      return status?.kind === "reading" || (status?.kind === "ready" && changesOf(status.draft).changed);
    }
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
  const waiting = chosen.filter((index) =>
    tracksOn ? trackStatuses[index]?.kind !== "ready" : statuses[index]?.kind !== "ready",
  ).length;
  const total = chosen.reduce((sum, index) => {
    if (tracksOn) {
      const status = trackStatuses[index];
      return sum + (status?.kind === "ready" ? (sizeOf(status.draft, status.contents) ?? 0) : 0);
    }
    const status = statuses[index];
    return sum + (status?.kind === "ready" ? (status.plan.estimated_size ?? 0) : 0);
  }, 0);
  const leftOut = statuses.filter((status) => status.kind === "nothing").length;
  const unreadable = statuses.filter((status) => status.kind === "unreadable").length;

  // How many were started, and whether they were copies of their tracks or conversions.
  const [added, setAdded] = useState({ count: 0, copies: false });
  const convert = () => {
    if (!choice || waiting > 0) return;
    if (tracksOn) {
      // A copy of each, with the tracks chosen, in an `Edited` folder beside it.
      for (const index of chosen) {
        const file = files[index];
        const status = trackStatuses[index];
        if (!file || status?.kind !== "ready") continue;
        const thumbnail = reads[index]?.data?.thumbnail ?? null;
        start({ url: file.key, title: file.name, thumbnail }, choice, null, null, null, editOf(status.draft));
      }
      setSelected(new Set());
      setAdded({ count: chosen.length, copies: true });
      openPanel("downloads");
      return;
    }
    for (const index of chosen) {
      const file = files[index];
      const status = statuses[index];
      if (!file || status?.kind !== "ready") continue;
      start({ url: file.key, title: file.name, thumbnail: status.video.thumbnail }, choice, null);
    }
    setSelected(new Set());
    setAdded({ count: chosen.length, copies: false });
    openPanel("downloads");
  };

  const folders = new Set(files.map((file) => file.folder));
  const onlyFolder = folders.size === 1 ? folderName(files[0]?.folder ?? "") : null;
  const originals = files.reduce((sum, file) => sum + file.size, 0);

  // What the tracks card's edge says: the languages the copies keep, or, until read, the files' tags.
  const tagsOf = (kind: "sound" | "subtitles") =>
    seasonFiles.length > 0
      ? seasonFiles.flatMap((file) =>
          file.draft.tracks.flatMap((track) => (track.keep && track.kind === kind && track.language ? [track.language] : [])),
        )
      : mkvs.flatMap(({ local }) => (kind === "sound" ? local.overview.sound_languages : local.overview.subtitle_languages));
  const removedAll = seasonFiles.reduce((sum, file) => sum + changesOf(file.draft).removed, 0);
  const deckTitle =
    mkvs.length === files.length ? `Tracks of all ${files.length} files` : `Tracks of ${mkvs.length} of ${files.length} files`;
  const deckSaid = [
    removedAll > 0 && `${removedAll} left out`,
    languagesLine(tagsOf("sound"), tagsOf("subtitles")) ?? "Choose which they keep, without converting",
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <>
      <div className="relative">
        <div className="relative z-10 flex gap-3.5 rounded-[1.25rem] border bg-card p-4 shadow-card">
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
        {mkvs.length > 0 && (
          <TracksDeck open={tracksOn} onOpenChange={openTracks} disabled={false} title={deckTitle} said={deckSaid}>
            <SeasonTracks
              files={seasonFiles}
              total={mkvs.length}
              reading={tracksReading}
              onDrafts={(changed) => setDrafts((now) => ({ ...now, ...Object.fromEntries(changed) }))}
              disabled={false}
            />
          </TracksDeck>
        )}
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
        <ul
          className={cn(
            "scrollbar-thin overflow-y-auto p-1.5 select-none",
            // Choosing tracks, a file can be opened in the list: it needs the room.
            tracksOn ? "max-h-[32rem]" : "max-h-72",
          )}
        >
          {shown.map(({ file, index }, position) => {
            const status = statuses[index] ?? { kind: "reading" };
            const trackStatus = trackStatuses[index] ?? { kind: "none" };
            const video = "video" in status ? status.video : null;
            const opened = tracksOn && openFile === file.key && trackStatus.kind === "ready";
            return (
              <motion.li
                key={file.key}
                initial={{ opacity: 0, y: 6, filter: "blur(3px)" }}
                animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                transition={{ ...spring, delay: Math.min(position, STAGGERED) * STAGGER }}
              >
                <FileRow
                  file={file}
                  video={video}
                  reading={status.kind === "reading"}
                  checked={selected.has(file.key) && selectable(index)}
                  off={!selectable(index)}
                  badge={tracksOn ? <TracksBadge status={trackStatus} queued={busy.has(file.key)} /> : <Badge status={status} />}
                  onClick={(event) => onRow(index, event)}
                  expand={
                    tracksOn && trackStatus.kind === "ready"
                      ? { open: opened, onToggle: () => setOpenFile(opened ? null : file.key) }
                      : undefined
                  }
                />
                <AnimatePresence initial={false}>
                  {opened && (
                    <motion.div
                      key="tracks"
                      initial={{ opacity: 0, height: 0 }}
                      animate={{ opacity: 1, height: "auto" }}
                      exit={{ opacity: 0, height: 0 }}
                      transition={spring}
                      className="overflow-hidden"
                    >
                      <div className="px-2 pb-2 pl-9">
                        <TracksPanel
                          fileKey={file.key}
                          version={file.version}
                          draft={trackStatus.draft}
                          onDraft={setDraft}
                          disabled={false}
                          embedded
                        />
                      </div>
                    </motion.div>
                  )}
                </AnimatePresence>
              </motion.li>
            );
          })}
          {shown.length === 0 && (
            <li className="px-3 py-6 text-center text-[13px] text-muted-foreground">Nothing in this list matches “{query}”.</li>
          )}
        </ul>
      </div>

      {/* Choosing the tracks replaces converting: its choices fold away. */}
      <AnimatePresence initial={false}>
        {!tracksOn && (
          <motion.div
            key="convert"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto", filter: "blur(0px)" }}
            exit={{ opacity: 0, height: 0, filter: "blur(3px)" }}
            transition={spring}
            className="space-y-3"
          >
            <div className="space-y-2 pt-1">
              <PresetArea summary={undefined} disabled={false} file />
              <AnimatePresence initial={false}>
                {customizing && (
                  <motion.div
                    key="custom"
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: "auto" }}
                    exit={{ opacity: 0, height: 0 }}
                    transition={spring}
                  >
                    <ManualPanel disabled={false} />
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
            <p className="px-1 text-[13px] text-pretty text-muted-foreground">
              Each file is planned from what it holds, and what is made goes beside it. Files that already are what you
              chose are left as they are.
            </p>
          </motion.div>
        )}
      </AnimatePresence>

      <Button
        size="lg"
        className="h-13 w-full rounded-2xl"
        disabled={chosen.length === 0 || waiting > 0 || !choice}
        onClick={convert}
      >
        {tracksOn ? <Download className="size-4.5" /> : <RefreshCw className="size-4.5" />}
        {tracksOn
          ? chosen.length === 0
            ? tracksReading > 0
              ? "Reading their tracks…"
              : "Nothing to change"
            : waiting > 0
              ? `Reading their tracks… ${chosen.length - waiting} of ${chosen.length}`
              : `Save ${chosen.length} ${chosen.length === 1 ? "copy" : "copies"}`
          : chosen.length === 0
            ? "Select files to convert"
            : waiting > 0
              ? `Reading the files… ${chosen.length - waiting} of ${chosen.length}`
              : `Convert ${chosen.length} ${chosen.length === 1 ? "file" : "files"}`}
      </Button>
      {added.count > 0 && (
        <p role="status" className="text-center text-[13px] text-muted-foreground">
          Started {added.count}{" "}
          {added.copies ? (added.count === 1 ? "copy" : "copies") : added.count === 1 ? "file" : "files"}. Follow them
          in{" "}
          <button
            type="button"
            onClick={() => openPanel("downloads")}
            className="font-medium text-primary underline-offset-2 hover:underline"
          >
            Activity
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
  video,
  reading,
  checked,
  off,
  badge,
  onClick,
  expand,
}: {
  file: LocalFile;
  video: VideoInfo | null;
  /** Not read yet: its picture is a placeholder. */
  reading: boolean;
  checked: boolean;
  /** Can't be chosen: nothing to do, can't be read, in the queue. */
  off: boolean;
  badge: ReactNode;
  onClick: (event: MouseEvent) => void;
  /** Its tracks can be opened under it, to choose by hand. */
  expand?: { open: boolean; onToggle: () => void };
}) {
  const sound = video?.formats[0]?.kind === "audio";
  return (
    <div className="flex items-center gap-1">
      <button
        type="button"
        role="checkbox"
        aria-checked={checked}
        aria-disabled={off}
        onClick={off ? undefined : onClick}
        className={cn(
          "flex min-w-0 flex-1 items-center gap-3 rounded-xl px-2 py-1.5 text-left transition-colors",
          off ? "cursor-default" : "hover:bg-accent",
        )}
      >
        <Box checked={checked} disabled={off} />
        {reading ? (
          <span className="skeleton aspect-video w-14 shrink-0 rounded-md" />
        ) : (
          <Thumb src={video?.thumbnail ?? null} empty={sound ? <AudioLines className="size-3.5" /> : undefined} />
        )}
        <span className={cn("min-w-0 flex-1 truncate text-[13px]", !checked && "text-muted-foreground")}>{file.name}</span>
        {badge}
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
          {[video?.duration != null && formatDuration(video.duration), formatBytes(file.size)].filter(Boolean).join(" · ")}
        </span>
      </button>
      {expand && (
        <Hint label={expand.open ? "Close its tracks" : "Its tracks, to choose by hand"}>
          <button
            type="button"
            aria-label={expand.open ? "Close its tracks" : "Its tracks"}
            aria-expanded={expand.open}
            onClick={expand.onToggle}
            className="grid size-8 shrink-0 place-items-center rounded-lg text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <ChevronDown className={cn("size-4 transition-transform", expand.open && "rotate-180")} />
          </button>
        </Hint>
      )}
    </div>
  );
}

/** What a file's copy keeps, while the season's tracks are chosen. */
function TracksBadge({ status, queued }: { status: TrackStatus; queued: boolean }) {
  if (queued) {
    return (
      <span className="shrink-0 rounded-md bg-secondary px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
        In queue
      </span>
    );
  }
  switch (status.kind) {
    case "none":
      return <span className="shrink-0 text-[11px] text-subtle-foreground">No tracks to choose</span>;
    case "reading":
      return <span className="shrink-0 text-[11px] text-subtle-foreground">Reading its tracks…</span>;
    case "failed":
      return (
        <Hint label={status.why} className="max-w-64 text-pretty">
          <span className="flex shrink-0 items-center gap-1 rounded-md bg-destructive-soft px-1.5 py-0.5 text-[11px] font-medium text-destructive">
            <TriangleAlert className="size-3" />
            Can’t be read
          </span>
        </Hint>
      );
    case "ready": {
      const unsure = toCheck(status.contents, status.draft);
      const { changed, kept, added } = changesOf(status.draft);
      return (
        <>
          {unsure.length > 0 && (
            <Hint
              label={
                <span className="block space-y-1">
                  {unsure.map((line) => (
                    <span key={line} className="block">
                      {line}
                    </span>
                  ))}
                  <span className="block">Kept, whatever the season’s choices: open its tracks to choose.</span>
                </span>
              }
              className="max-w-72 text-pretty"
            >
              <span>
                <UnsureMark lines={unsure} />
              </span>
            </Hint>
          )}
          {changed ? (
            <span className="shrink-0 rounded-md bg-secondary px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground tabular-nums">
              Keeps {kept} of {status.draft.before.length}
              {added > 0 && `, adds ${added}`}
            </span>
          ) : (
            <span className="flex shrink-0 items-center gap-1 rounded-md bg-success/12 px-1.5 py-0.5 text-[11px] font-medium text-success">
              <Check className="size-3" strokeWidth={2.5} />
              Nothing to change
            </span>
          )}
        </>
      );
    }
  }
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
    case "ready": {
      // What a single file says above its button, said here before Convert:
      // its subtitle tracks the file type can't hold, say.
      const warnings = status.plan.hints.filter((hint) => hint.level === "warning").map((hint) => hint.text);
      if (warnings.length === 0) return null;
      return (
        <Hint label={warnings.join(" ")} className="max-w-64 text-pretty">
          <span className="flex shrink-0 items-center gap-1 rounded-md bg-destructive-soft px-1.5 py-0.5 text-[11px] font-medium text-foreground">
            <TriangleAlert className="size-3 text-destructive" />
            {warnings.some((text) => text.includes("subtitle")) ? "Subtitles left out" : "Note"}
          </span>
        </Hint>
      );
    }
    default:
      return null;
  }
}
