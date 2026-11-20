import { useQueries, useQuery } from "@tanstack/react-query";
import { Check, ChevronRight, ImageOff, ListPlus, ListVideo, Search, X } from "lucide-react";
import { Fragment, type MouseEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { Button } from "@/components/ui/button";
import { PresetArea } from "@/features/presets/PresetArea";
import { formatBytes, formatDuration } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useChoice, useChoices } from "@/stores/choices";
import { type Download, isActive, useDownloads } from "@/stores/downloads";
import { useTrim } from "@/stores/trim";
import { useUi } from "@/stores/ui";
import type { PlaylistEntry, PlaylistInfo } from "@/types";

/**
 * How many selected videos are looked up for the total size. The rest are
 * scaled from them by length: looking up every video of a long playlist took
 * minutes and sent YouTube a request per video.
 */
const SAMPLE = 15;

/** Long enough a list that finding a video by name helps. */
const SEARCH_FROM = 10;

/** Where the folder and numbering choices are remembered (only the interface uses them). */
const OPTIONS_KEY = "siphon.playlistOptions";

interface Options {
  /** Save the videos in a folder named after the playlist. */
  folder: boolean;
  /** Put the video's position in front of its name: "07 - Title". */
  number: boolean;
}

function loadOptions(): Options {
  try {
    const saved = JSON.parse(localStorage.getItem(OPTIONS_KEY) ?? "null") as Partial<Options> | null;
    return { folder: saved?.folder === true, number: saved?.number === true };
  } catch {
    return { folder: false, number: false };
  }
}

/** What Siphon already has of a playlist video. */
type Have = "downloaded" | "queued";

/**
 * A playlist or channel: pick which videos to download, then add them to the
 * queue in one go. A channel's videos are grouped by tab (Videos, Live, Shorts),
 * and links to other playlists open them. Videos already downloaded or in the
 * queue are marked and left out, so pasting a playlist again fetches only
 * what's new.
 */
export function PlaylistPane({ playlist, onOpen }: { playlist: PlaylistInfo; onOpen: (url: string) => void }) {
  const choice = useChoice();
  const start = useDownloads((state) => state.start);
  const openPanel = useUi((state) => state.openPanel);

  const { entries } = playlist;
  // A stream on air or scheduled is opened to record, not queued with the rest.
  const videos = useMemo(() => entries.filter((entry) => !opens(entry)), [entries]);
  const have = useHave(videos);

  // A channel starts with its first tab (Videos) selected, not its Live and Shorts too,
  // and nothing that is already downloaded or in the queue.
  const [selected, setSelected] = useState(() => {
    const first = videos[0]?.section ?? null;
    const known = haveNow(videos);
    return new Set(videos.filter((entry) => entry.section === first && !known.has(entry.url)).map((entry) => entry.url));
  });
  // Until the user picks for themselves, videos whose file turns out to be gone are selected again.
  const touched = useRef(false);
  const restored = useRef(false);
  useEffect(() => {
    if (!have.checked || restored.current) return;
    restored.current = true;
    if (touched.current) return;
    const first = videos[0]?.section ?? null;
    const missing = videos.filter(
      (entry) => entry.section === first && !have.of.has(entry.url) && haveNow([entry]).has(entry.url),
    );
    if (missing.length > 0) setSelected((current) => new Set([...current, ...missing.map((entry) => entry.url)]));
  }, [have, videos]);

  const [added, setAdded] = useState(0);
  const [options, setOptions] = useState(loadOptions);
  // From the latest options, so two quick changes can't undo each other.
  const changeOption = (change: Partial<Options>) =>
    setOptions((current) => {
      const next = { ...current, ...change };
      try {
        localStorage.setItem(OPTIONS_KEY, JSON.stringify(next));
      } catch {
        // Not remembered this time; the choice still applies.
      }
      return next;
    });

  // Stream picks and trim ranges belong to a single video, not to a playlist.
  useEffect(() => {
    useChoices.getState().resetForVideo();
    useTrim.getState().reset();
  }, []);

  const [query, setQuery] = useState("");
  const visible = useMemo(() => {
    const words = fold(query).split(/\s+/).filter(Boolean);
    return words.length === 0 ? entries : entries.filter((entry) => words.every((word) => fold(entry.title).includes(word)));
  }, [entries, query]);
  const visibleVideos = visible.filter((entry) => !opens(entry));
  // A video already in the queue can't be added again.
  const selectable = (entry: PlaylistEntry) => have.of.get(entry.url) !== "queued";

  const toggle = (urls: string[], on: boolean) => {
    touched.current = true;
    setSelected((current) => {
      const next = new Set(current);
      for (const url of urls) {
        if (on) next.add(url);
        else next.delete(url);
      }
      return next;
    });
  };

  // Shift-click ticks or unticks everything between the last row clicked and this one.
  const anchor = useRef<string | null>(null);
  const onRow = (entry: PlaylistEntry, event: MouseEvent) => {
    const on = !selected.has(entry.url);
    const from = anchor.current ? visibleVideos.findIndex((video) => video.url === anchor.current) : -1;
    const to = visibleVideos.indexOf(entry);
    if (event.shiftKey && from >= 0 && to >= 0) {
      const range = visibleVideos.slice(Math.min(from, to), Math.max(from, to) + 1);
      toggle(range.filter(selectable).map((video) => video.url), on);
    } else {
      toggle([entry.url], on);
    }
    anchor.current = entry.url;
  };

  const shownSelectable = visibleVideos.filter(selectable);
  const allShown = shownSelectable.length > 0 && shownSelectable.every((entry) => selected.has(entry.url));

  const chosen = videos.filter((entry) => selected.has(entry.url) && selectable(entry));
  const sample = chosen.slice(0, SAMPLE);
  const estimates = useQueries({
    queries: sample.map((entry) => ({
      queryKey: ["estimate", entry.url, choice],
      queryFn: () => (choice ? ipc.estimateDownload(entry.url, choice) : Promise.resolve(null)),
      enabled: choice !== null,
      staleTime: Number.POSITIVE_INFINITY,
    })),
  });
  const size = totalSize(chosen, sample, estimates.map((estimate) => (estimate.isFetched ? (estimate.data ?? null) : undefined)));

  const addToQueue = () => {
    if (!choice) return;
    for (const entry of chosen) {
      start({ url: entry.url, title: entry.title, thumbnail: entry.thumbnail }, choice, null, {
        folder: options.folder ? playlist.title : null,
        number: options.number ? [videos.indexOf(entry) + 1, videos.length] : null,
      });
    }
    // They're in the queue now; nothing is left selected to add twice.
    touched.current = true;
    setSelected(new Set());
    setAdded(chosen.length);
    openPanel("downloads");
  };

  // Grouped by section, in the order the site lists them.
  const sections: { name: string | null; entries: PlaylistEntry[] }[] = [];
  for (const entry of visible) {
    const last = sections.at(-1);
    if (last && last.name === entry.section) last.entries.push(entry);
    else sections.push({ name: entry.section, entries: [entry] });
  }
  const links = entries.length - videos.length;
  const downloaded = videos.filter((entry) => have.of.get(entry.url) === "downloaded").length;
  const queued = videos.filter((entry) => have.of.get(entry.url) === "queued").length;

  return (
    <>
      <div className="flex gap-3.5 rounded-[1.25rem] border bg-card p-4 shadow-card">
        <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary-soft text-primary">
          <ListVideo className="size-4.5" />
        </div>
        <div className="min-w-0">
          <h2 data-selectable className="truncate font-semibold tracking-tight">
            {playlist.title}
          </h2>
          <p className="mt-0.5 truncate text-[13px] text-muted-foreground">
            {[
              playlist.channel,
              videos.length > 0 && `${videos.length} ${videos.length === 1 ? "video" : "videos"}`,
              links > 0 && `${links} ${links === 1 ? "playlist" : "playlists"}`,
              downloaded > 0 && `${downloaded} downloaded`,
              queued > 0 && `${queued} in the queue`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
      </div>

      <div className="overflow-hidden rounded-[1.25rem] border bg-card shadow-card">
        {videos.length > 0 && (
          <div className="flex items-center justify-between gap-2 border-b py-2 pr-2 pl-4">
            <span className="min-w-0 truncate text-[13px] text-muted-foreground tabular-nums">
              {chosen.length} of {videos.length} selected
              {chosen.length > 0 && choice && <span className="text-subtle-foreground"> · {size}</span>}
            </span>
            {shownSelectable.length > 0 && (
              <Button
                variant="ghost"
                size="sm"
                className="shrink-0"
                onClick={() => toggle(shownSelectable.map((entry) => entry.url), !allShown)}
              >
                {allShown ? "Select none" : query ? "Select all shown" : "Select all"}
              </Button>
            )}
          </div>
        )}
        {entries.length >= SEARCH_FROM && (
          <label className="flex items-center gap-2 border-b px-4 py-1.5">
            <Search className="size-3.5 shrink-0 text-subtle-foreground" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => event.key === "Escape" && query && (event.stopPropagation(), setQuery(""))}
              placeholder="Find by title"
              aria-label="Find by title"
              spellCheck={false}
              className="h-7 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-subtle-foreground"
            />
            {query && (
              <>
                <span className="shrink-0 text-xs text-subtle-foreground tabular-nums">{visible.length} found</span>
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
          {sections.map((section, index) => {
            const urls = section.entries.filter((entry) => !opens(entry) && selectable(entry)).map((entry) => entry.url);
            const on = urls.length > 0 && urls.every((url) => selected.has(url));
            return (
              <Fragment key={`${section.name}:${index}`}>
                {section.name && (
                  <li className="sticky -top-1.5 z-10 -mx-1.5 flex items-center justify-between bg-card px-3.5 pt-2 pb-1">
                    <span className="text-xs font-medium text-muted-foreground">
                      {section.name} <span className="text-subtle-foreground tabular-nums">· {section.entries.length}</span>
                    </span>
                    {urls.length > 0 && (
                      <button
                        type="button"
                        onClick={() => toggle(urls, !on)}
                        className="rounded-md px-1.5 py-0.5 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                      >
                        {on ? "Select none" : "Select all"}
                      </button>
                    )}
                  </li>
                )}
                {section.entries.map((entry) => (
                  <li key={entry.url}>
                    {opens(entry) ? (
                      <LinkRow entry={entry} onOpen={() => onOpen(entry.url)} />
                    ) : (
                      <EntryRow
                        entry={entry}
                        index={videos.indexOf(entry)}
                        checked={selected.has(entry.url) && selectable(entry)}
                        have={have.of.get(entry.url)}
                        onClick={(event) => onRow(entry, event)}
                      />
                    )}
                  </li>
                ))}
              </Fragment>
            );
          })}
          {visible.length === 0 && (
            <li className="px-3 py-6 text-center text-[13px] text-muted-foreground">Nothing in this list matches “{query}”.</li>
          )}
        </ul>
      </div>

      {videos.length === 0 ? (
        <p className="px-1 text-[13px] text-pretty text-muted-foreground">Open a playlist to choose its videos.</p>
      ) : (
        <>
          <div className="flex flex-wrap gap-x-5 gap-y-1 px-1">
            <Option checked={options.folder} onChange={(folder) => changeOption({ folder })}>
              Save in a folder named <span className="font-medium text-foreground">{playlist.title}</span>
            </Option>
            <Option checked={options.number} onChange={(number) => changeOption({ number })}>
              Number the files in playlist order <span className="text-subtle-foreground">(01 - Title)</span>
            </Option>
          </div>

          <div className="pt-1">
            <PresetArea summary={undefined} disabled={false} playlist subtitles="playlist" />
          </div>
          <p className="px-1 text-[13px] text-pretty text-muted-foreground">
            The best streams and format for each video are worked out from what it offers when its turn comes.
          </p>

          <Button size="lg" className="h-13 w-full rounded-2xl" disabled={chosen.length === 0 || !choice} onClick={addToQueue}>
            <ListPlus className="size-4.5" />
            {chosen.length === 0
              ? "Select videos to download"
              : `Add ${chosen.length} ${chosen.length === 1 ? "video" : "videos"} to the queue`}
          </Button>
          {added > 0 && (
            <p role="status" className="text-center text-[13px] text-muted-foreground">
              Added {added} {added === 1 ? "video" : "videos"}. Follow them in{" "}
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
      )}
    </>
  );
}

/** Lower case, without accents, for matching titles: "Café" is found by "cafe". */
const fold = (text: string) =>
  text
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();

/** What the download list says about these videos right now: queued wins over downloaded. */
function haveNow(videos: PlaylistEntry[]): Map<string, Have> {
  return summarize(videos, Object.values(useDownloads.getState().byId));
}

function summarize(videos: PlaylistEntry[], downloads: Download[]): Map<string, Have> {
  const urls = new Set(videos.map((entry) => entry.url));
  const result = new Map<string, Have>();
  for (const download of downloads) {
    if (!urls.has(download.url)) continue;
    if (isActive(download) || download.status === "paused") result.set(download.url, "queued");
    else if (download.status === "finished" && download.path && download.trim === null && !result.has(download.url)) {
      result.set(download.url, "downloaded");
    }
  }
  return result;
}

/**
 * Which videos are downloaded (the whole video, and the file still there) or
 * in the queue, kept up to date as downloads start and finish. `checked` turns
 * true once the files have been looked for.
 */
function useHave(videos: PlaylistEntry[]): { of: Map<string, Have>; checked: boolean } {
  const urls = useMemo(() => new Set(videos.map((entry) => entry.url)), [videos]);
  const mine = useDownloads(
    useShallow((state) =>
      state.order.map((id) => state.byId[id]).filter((download): download is Download => download !== undefined && urls.has(download.url)),
    ),
  );
  const paths = useMemo(
    () =>
      mine
        .filter((download) => download.status === "finished" && download.path && download.trim === null)
        .map((download) => download.path as string),
    [mine],
  );
  const present = useQuery({
    queryKey: ["files-present", paths],
    queryFn: () => ipc.filesPresent(paths),
    enabled: paths.length > 0,
    staleTime: 0,
  });
  return useMemo(() => {
    const gone = new Set(paths.filter((_, index) => present.data?.[index] === false));
    const of = summarize(
      videos,
      // Until the files have been checked, a finished download counts; a gone one never does.
      mine.filter((download) => !(download.path && gone.has(download.path))),
    );
    // Checked by a look made after this list opened, not one remembered from before (a file may be gone since).
    return { of, checked: paths.length === 0 || present.isFetchedAfterMount };
  }, [videos, mine, paths, present.data, present.isFetchedAfterMount]);
}

/**
 * The size line: the exact total for a short selection; for a longer one,
 * the sample's size per second of video scaled to the rest by their length
 * (or the sample's average per video where a length is unknown).
 * `sizes` holds the sample's estimates in order: undefined while still being
 * looked up, null when it couldn't be.
 */
function totalSize(chosen: PlaylistEntry[], sample: PlaylistEntry[], sizes: (number | null | undefined)[]): string {
  const known = sample.flatMap((entry, index) => {
    const size = sizes[index];
    return size != null ? [{ entry, size }] : [];
  });
  const pending = sizes.filter((size) => size === undefined).length;
  const sum = known.reduce((total, item) => total + item.size, 0);

  if (chosen.length <= sample.length) {
    if (known.length === 0 && pending > 0) return "working out the total size…";
    return pending > 0 ? `≈ ${formatBytes(sum)} so far (${sample.length - pending} of ${sample.length} checked)` : `≈ ${formatBytes(sum)}`;
  }
  if (pending > 0 || known.length === 0) {
    return pending > 0 ? "working out the total size…" : "total size unknown";
  }
  const timed = known.filter((item) => item.entry.duration);
  const seconds = timed.reduce((total, item) => total + (item.entry.duration ?? 0), 0);
  const perSecond = seconds > 0 ? timed.reduce((total, item) => total + item.size, 0) / seconds : null;
  const perVideo = sum / known.length;
  const sampled = new Set(known.map((item) => item.entry.url));
  const rest = chosen
    .filter((entry) => !sampled.has(entry.url))
    .reduce((total, entry) => total + (perSecond && entry.duration ? perSecond * entry.duration : perVideo), 0);
  return `≈ ${formatBytes(sum + rest)}, estimated from ${known.length} videos`;
}

/** The square tick box the list and its options share. */
function Box({ checked, disabled = false }: { checked: boolean; disabled?: boolean }) {
  return (
    <span
      className={cn(
        "grid size-4.5 shrink-0 place-items-center rounded-md border transition-colors duration-150",
        checked ? "border-primary bg-primary text-primary-foreground" : "border-border-strong",
        disabled && "opacity-40",
      )}
    >
      {checked && <Check className="size-3" strokeWidth={3} />}
    </span>
  );
}

function Option({ checked, onChange, children }: { checked: boolean; onChange: (checked: boolean) => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className="flex min-w-0 items-center gap-2 rounded-lg py-1 text-left text-[13px] text-muted-foreground transition-colors hover:text-foreground"
    >
      <Box checked={checked} />
      <span className="min-w-0 truncate">{children}</span>
    </button>
  );
}

function EntryRow({
  entry,
  index,
  checked,
  have,
  onClick,
}: {
  entry: PlaylistEntry;
  index: number;
  checked: boolean;
  have: Have | undefined;
  onClick: (event: MouseEvent) => void;
}) {
  const queued = have === "queued";
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-disabled={queued}
      onClick={queued ? undefined : onClick}
      className={cn(
        "flex w-full items-center gap-3 rounded-xl px-2 py-1.5 text-left transition-colors",
        queued ? "cursor-default" : "hover:bg-accent",
      )}
    >
      <Box checked={checked} disabled={queued} />
      <span className="w-6 shrink-0 text-right text-xs text-subtle-foreground tabular-nums">{index + 1}</span>
      <Thumb src={entry.thumbnail} />
      <span className={cn("min-w-0 flex-1 truncate text-[13px]", !checked && "text-muted-foreground")}>
        {entry.title}
      </span>
      {have && (
        <span
          className={cn(
            "flex shrink-0 items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] font-medium",
            have === "downloaded" ? "bg-success/12 text-success" : "bg-secondary text-muted-foreground",
          )}
        >
          {have === "downloaded" && <Check className="size-3" strokeWidth={2.5} />}
          {have === "downloaded" ? "Downloaded" : "In queue"}
        </span>
      )}
      {entry.duration != null && (
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{formatDuration(entry.duration)}</span>
      )}
    </button>
  );
}

/** Entries that are opened rather than queued: another playlist, or a stream on air or scheduled. */
const opens = (entry: PlaylistEntry) => entry.is_playlist || entry.live !== null;

/**
 * A link to another playlist, opened to choose from its videos; or a stream
 * that is live or scheduled, opened to record it.
 */
function LinkRow({ entry, onOpen }: { entry: PlaylistEntry; onOpen: () => void }) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group flex w-full items-center gap-3 rounded-xl px-2 py-1.5 text-left transition-colors hover:bg-accent"
    >
      {entry.live ? (
        <span className="grid size-4.5 shrink-0 place-items-center">
          <span
            aria-hidden
            className={cn("size-2.5 rounded-full", entry.live === "live" ? "bg-live" : "border-2 border-live")}
          />
        </span>
      ) : (
        <ListVideo className="size-4.5 shrink-0 text-primary" />
      )}
      <span className="w-6 shrink-0" />
      <Thumb src={entry.thumbnail} />
      <span className="min-w-0 flex-1 truncate text-[13px]">{entry.title}</span>
      {entry.live && (
        <span
          className={cn(
            "shrink-0 rounded-md px-1.5 py-0.5 text-[11px] font-semibold tracking-wide",
            entry.live === "live" ? "bg-live-soft text-live" : "bg-secondary text-muted-foreground",
          )}
        >
          {entry.live === "live" ? "LIVE" : "UPCOMING"}
        </span>
      )}
      <span className="flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground group-hover:text-foreground">
        {entry.live ? "Record" : "Open"} <ChevronRight className="size-3.5" />
      </span>
    </button>
  );
}

function Thumb({ src }: { src: string | null }) {
  return (
    <span className="relative aspect-video w-14 shrink-0 overflow-hidden rounded-md bg-muted">
      {src ? (
        <img src={src} alt="" loading="lazy" draggable={false} referrerPolicy="no-referrer" className="size-full object-cover" />
      ) : (
        <span className="grid size-full place-items-center text-subtle-foreground">
          <ImageOff className="size-3" />
        </span>
      )}
    </span>
  );
}
