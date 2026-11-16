import { Check, ImageOff, ListPlus, ListVideo } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { PresetPicker } from "@/features/options/PresetPicker";
import { QualityPicker } from "@/features/options/QualityPicker";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useDownloads } from "@/stores/downloads";
import { useDownloadOptions, useOptions } from "@/stores/options";
import { useTrim } from "@/stores/trim";
import { useUi } from "@/stores/ui";
import type { PlaylistEntry, PlaylistInfo } from "@/types";

/** A playlist: pick which videos to download, then add them to the queue in one go. */
export function PlaylistPane({ playlist }: { playlist: PlaylistInfo }) {
  const options = useDownloadOptions();
  const mode = useOptions((state) => state.mode);
  const start = useDownloads((state) => state.start);
  const openPanel = useUi((state) => state.openPanel);

  const { entries } = playlist;
  const [selected, setSelected] = useState(() => new Set(entries.map((entry) => entry.url)));
  const [added, setAdded] = useState(0);

  // Stream picks and trim ranges belong to a single video, not to a playlist.
  useEffect(() => {
    useOptions.getState().resetForVideo();
    useTrim.getState().reset();
  }, []);

  const toggle = (url: string) =>
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(url)) next.add(url);
      return next;
    });
  const allSelected = selected.size === entries.length;

  const addToQueue = () => {
    const chosen = entries.filter((entry) => selected.has(entry.url));
    for (const entry of chosen) {
      start({ url: entry.url, title: entry.title, thumbnail: entry.thumbnail }, options, null);
    }
    setAdded(chosen.length);
    openPanel("downloads");
  };

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
            {[playlist.channel, `${entries.length} videos`].filter(Boolean).join(" · ")}
          </p>
        </div>
      </div>

      <div className="overflow-hidden rounded-[1.25rem] border bg-card shadow-card">
        <div className="flex items-center justify-between border-b py-2 pr-2 pl-4">
          <span className="text-[13px] text-muted-foreground tabular-nums">
            {selected.size} of {entries.length} selected
          </span>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setSelected(allSelected ? new Set() : new Set(entries.map((entry) => entry.url)))}
          >
            {allSelected ? "Select none" : "Select all"}
          </Button>
        </div>
        <ul className="scrollbar-thin max-h-72 overflow-y-auto p-1.5">
          {entries.map((entry, index) => (
            <li key={entry.url}>
              <EntryRow entry={entry} index={index} checked={selected.has(entry.url)} onToggle={() => toggle(entry.url)} />
            </li>
          ))}
        </ul>
      </div>

      <div className="space-y-2 pt-1">
        <PresetPicker disabled={false} />
        {/* No formats are known until each video is looked up, so only the upload limits apply here. */}
        {mode === "preset" && <QualityPicker plan={undefined} disabled={false} />}
      </div>
      <p className="px-1 text-[13px] text-pretty text-muted-foreground">
        The best quality and format for{" "}
        {mode === "manual" ? "your custom settings" : "this preset"} are worked out for each video when its turn comes.
      </p>

      <Button size="lg" className="h-13 w-full rounded-2xl" disabled={selected.size === 0} onClick={addToQueue}>
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
  );
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
      <span className="w-5 shrink-0 text-right text-xs text-subtle-foreground tabular-nums">{index + 1}</span>
      <span className="relative aspect-video w-14 shrink-0 overflow-hidden rounded-md bg-muted">
        {entry.thumbnail ? (
          <img
            src={entry.thumbnail}
            alt=""
            loading="lazy"
            draggable={false}
            referrerPolicy="no-referrer"
            className="size-full object-cover"
          />
        ) : (
          <span className="grid size-full place-items-center text-subtle-foreground">
            <ImageOff className="size-3" />
          </span>
        )}
      </span>
      <span className={cn("min-w-0 flex-1 truncate text-[13px]", !checked && "text-muted-foreground")}>
        {entry.title}
      </span>
      {entry.duration != null && (
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{formatDuration(entry.duration)}</span>
      )}
    </button>
  );
}
