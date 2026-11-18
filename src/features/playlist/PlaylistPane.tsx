import { useQueries } from "@tanstack/react-query";
import { Check, ChevronRight, ImageOff, ListPlus, ListVideo } from "lucide-react";
import { Fragment, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { PresetArea } from "@/features/presets/PresetArea";
import { formatBytes, formatDuration } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useChoice, useChoices } from "@/stores/choices";
import { useDownloads } from "@/stores/downloads";
import { useTrim } from "@/stores/trim";
import { useUi } from "@/stores/ui";
import type { PlaylistEntry, PlaylistInfo } from "@/types";

/**
 * How many selected videos are looked up for the total size. The rest are
 * scaled from them by length: looking up every video of a long playlist took
 * minutes and sent YouTube a request per video.
 */
const SAMPLE = 15;

/**
 * A playlist or channel: pick which videos to download, then add them to the
 * queue in one go. A channel's videos are grouped by tab (Videos, Live, Shorts),
 * and links to other playlists open them.
 */
export function PlaylistPane({ playlist, onOpen }: { playlist: PlaylistInfo; onOpen: (url: string) => void }) {
  const choice = useChoice();
  const start = useDownloads((state) => state.start);
  const openPanel = useUi((state) => state.openPanel);

  const { entries } = playlist;
  const videos = entries.filter((entry) => !entry.is_playlist);
  // A channel starts with its first tab (Videos) selected, not its Live and Shorts too.
  const [selected, setSelected] = useState(() => {
    const first = videos[0]?.section ?? null;
    return new Set(videos.filter((entry) => entry.section === first).map((entry) => entry.url));
  });
  const [added, setAdded] = useState(0);

  // Stream picks and trim ranges belong to a single video, not to a playlist.
  useEffect(() => {
    useChoices.getState().resetForVideo();
    useTrim.getState().reset();
  }, []);

  const toggle = (urls: string[], on: boolean) =>
    setSelected((current) => {
      const next = new Set(current);
      for (const url of urls) {
        if (on) next.add(url);
        else next.delete(url);
      }
      return next;
    });
  const allSelected = videos.length > 0 && videos.every((entry) => selected.has(entry.url));

  const chosen = videos.filter((entry) => selected.has(entry.url));
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
      start({ url: entry.url, title: entry.title, thumbnail: entry.thumbnail }, choice, null);
    }
    setAdded(chosen.length);
    openPanel("downloads");
  };

  // Grouped by section, in the order the site lists them.
  const sections: { name: string | null; entries: PlaylistEntry[] }[] = [];
  for (const entry of entries) {
    const last = sections.at(-1);
    if (last && last.name === entry.section) last.entries.push(entry);
    else sections.push({ name: entry.section, entries: [entry] });
  }
  const links = entries.length - videos.length;

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
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
      </div>

      <div className="overflow-hidden rounded-[1.25rem] border bg-card shadow-card">
        {videos.length > 0 && (
          <div className="flex items-center justify-between border-b py-2 pr-2 pl-4">
            <span className="text-[13px] text-muted-foreground tabular-nums">
              {selected.size} of {videos.length} selected
              {chosen.length > 0 && choice && <span className="text-subtle-foreground"> · {size}</span>}
            </span>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => toggle(videos.map((entry) => entry.url), !allSelected)}
            >
              {allSelected ? "Select none" : "Select all"}
            </Button>
          </div>
        )}
        <ul className="scrollbar-thin max-h-72 overflow-y-auto p-1.5">
          {sections.map((section, index) => {
            const urls = section.entries.filter((entry) => !entry.is_playlist).map((entry) => entry.url);
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
                    {entry.is_playlist ? (
                      <LinkRow entry={entry} onOpen={() => onOpen(entry.url)} />
                    ) : (
                      <EntryRow
                        entry={entry}
                        index={videos.indexOf(entry)}
                        checked={selected.has(entry.url)}
                        onToggle={() => toggle([entry.url], !selected.has(entry.url))}
                      />
                    )}
                  </li>
                ))}
              </Fragment>
            );
          })}
        </ul>
      </div>

      {videos.length === 0 ? (
        <p className="px-1 text-[13px] text-pretty text-muted-foreground">Open a playlist to choose its videos.</p>
      ) : (
        <>
          <div className="pt-1">
            <PresetArea summary={undefined} disabled={false} playlist />
          </div>
          <p className="px-1 text-[13px] text-pretty text-muted-foreground">
            The best streams and format for each video are worked out from what it offers when its turn comes.
          </p>

          <Button size="lg" className="h-13 w-full rounded-2xl" disabled={selected.size === 0 || !choice} onClick={addToQueue}>
            <ListPlus className="size-4.5" />
            {selected.size === 0
              ? "Select videos to download"
              : `Add ${selected.size} ${selected.size === 1 ? "video" : "videos"} to the queue`}
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

function EntryRow({
  entry,
  index,
  checked,
  onToggle,
}: {
  entry: PlaylistEntry;
  index: number;
  checked: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      onClick={onToggle}
      className="flex w-full items-center gap-3 rounded-xl px-2 py-1.5 text-left transition-colors hover:bg-accent"
    >
      <span
        className={cn(
          "grid size-4.5 shrink-0 place-items-center rounded-md border transition-colors duration-150",
          checked ? "border-primary bg-primary text-primary-foreground" : "border-border-strong",
        )}
      >
        {checked && <Check className="size-3" strokeWidth={3} />}
      </span>
      <span className="w-6 shrink-0 text-right text-xs text-subtle-foreground tabular-nums">{index + 1}</span>
      <Thumb src={entry.thumbnail} />
      <span className={cn("min-w-0 flex-1 truncate text-[13px]", !checked && "text-muted-foreground")}>
        {entry.title}
      </span>
      {entry.duration != null && (
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{formatDuration(entry.duration)}</span>
      )}
    </button>
  );
}

/** A link to another playlist: opens it, to choose from its videos. */
function LinkRow({ entry, onOpen }: { entry: PlaylistEntry; onOpen: () => void }) {
  return (
    <button
      type="button"
      onClick={onOpen}
      className="group flex w-full items-center gap-3 rounded-xl px-2 py-1.5 text-left transition-colors hover:bg-accent"
    >
      <ListVideo className="size-4.5 shrink-0 text-primary" />
      <span className="w-6 shrink-0" />
      <Thumb src={entry.thumbnail} />
      <span className="min-w-0 flex-1 truncate text-[13px]">{entry.title}</span>
      <span className="flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground group-hover:text-foreground">
        Open <ChevronRight className="size-3.5" />
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
