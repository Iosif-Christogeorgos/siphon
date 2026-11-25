import { ArrowLeft, ChevronDown, Layers } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { languageName } from "@/lib/languages";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { TrackOverview } from "@/types";
import { changesOf, type Draft } from "./draft";
import { TracksPanel } from "./TracksPanel";

/** "Sound in Greek and English", or in "5 languages" past three; null for none known. */
function inLanguages(what: string, tags: string[]): string | null {
  const names = [...new Set(tags.map((tag) => languageName(tag)))];
  if (names.length === 0) return null;
  if (names.length > 3) return `${what} in ${names.length} languages`;
  return `${what} in ${names.slice(0, -1).join(", ")}${names.length > 1 ? " and " : ""}${names.at(-1)}`;
}

/**
 * An MKV's tracks, as a card tucked behind the file's card with its edge
 * showing (docs/mkv-editor.md). Opened, it slides out from under, the file's
 * card above it shrinks to a slim header, and the tracks fill it; converting
 * is put away meanwhile, by the pane.
 */
export function TracksCard({
  fileKey,
  version,
  overview,
  open,
  onOpenChange,
  draft,
  onDraft,
  disabled,
}: {
  fileKey: string;
  version: string;
  /** What the file's tags say it holds: what the edge says until its tracks are read. */
  overview: TrackOverview;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  draft: Draft | null;
  onDraft: (draft: Draft) => void;
  /** A copy is being made: nothing can change meanwhile. */
  disabled: boolean;
}) {
  // It peeks out once, when the file opens, to show it can be pulled; not again after.
  const [fresh, setFresh] = useState(true);
  const total = overview.video + overview.sound + overview.subtitles;
  // Once read and chosen from, the edge says what the copy keeps.
  const current = draft !== null && draft.key === fileKey && draft.version === version ? draft : null;
  const changes = current ? changesOf(current) : null;
  const kept = current?.tracks.filter((track) => track.keep) ?? null;
  const tagsOf = (kind: "sound" | "subtitles") =>
    kept
      ? kept.filter((track) => track.kind === kind && track.language).map((track) => track.language as string)
      : kind === "sound"
        ? overview.sound_languages
        : overview.subtitle_languages;
  const title =
    changes && changes.removed > 0 ? `${changes.kept} of ${total} tracks kept` : `${total} tracks`;
  const sound = [...new Set(tagsOf("sound"))];
  const subtitles = [...new Set(tagsOf("subtitles"))];
  const same = sound.length > 0 && sound.join() === subtitles.join();
  const said =
    (same
      ? inLanguages("Sound and subtitles", sound)
      : [inLanguages("Sound", sound), inLanguages("subtitles", subtitles)].filter((part) => part !== null).join(" · ")) ||
    "Choose which it keeps, without converting";
  const toggle = (to: boolean) => {
    setFresh(false);
    onOpenChange(to);
  };

  return (
    <div
      className={cn(
        "relative z-0 border transition-[margin,border-radius,background-color,border-color,box-shadow] duration-700 ease-drawer motion-reduce:transition-none",
        open
          ? "mx-0 mt-3.5 rounded-[1.25rem] border-primary/25 bg-card shadow-card"
          : cn(
              // Its top edge is behind the file's card; only its lower part shows.
              "group/tracks mx-3.5 -mt-7 rounded-b-[1.125rem] bg-card-behind shadow-card",
              !disabled && "hover:-mt-[1.375rem]",
              fresh && "motion-safe:animate-peek",
            ),
      )}
    >
      <div
        className={cn(
          "grid px-3 transition-[padding] duration-700 ease-drawer motion-reduce:transition-none",
          open ? "pt-0" : "pt-7",
        )}
      >
        <button
          type="button"
          onClick={() => toggle(true)}
          disabled={disabled}
          aria-expanded={open}
          inert={open}
          className={cn(
            "col-start-1 row-start-1 flex h-15 w-full min-w-0 items-center gap-3 rounded-xl px-1 text-left transition-[opacity,translate] duration-300 outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:opacity-60",
            open ? "pointer-events-none -translate-y-2.5 opacity-0" : "opacity-100",
          )}
        >
          <LayersTile spread={false} />
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="text-[15px] font-semibold">{title}</span>
            <span className="truncate text-[13px] text-muted-foreground">{said}</span>
          </span>
          <span className="relative ml-auto inline-flex h-9 shrink-0 items-center gap-1.5 overflow-hidden rounded-[0.625rem] border border-primary/30 bg-primary-soft px-3.5 text-[13.5px] font-semibold text-primary transition-colors group-hover/tracks:border-primary/50 group-hover/tracks:bg-primary/20">
            Edit tracks
            <ChevronDown className="size-4" />
          </span>
        </button>

        <div
          inert={!open}
          className={cn(
            "col-start-1 row-start-1 flex h-16 min-w-0 items-center gap-3 px-1 transition-[opacity,translate] duration-300",
            open ? "opacity-100 delay-150" : "pointer-events-none translate-y-2.5 opacity-0",
          )}
        >
          <LayersTile spread />
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="text-[15px] font-semibold">Tracks</span>
            <span className="truncate text-[13px] text-muted-foreground">
              Every track is copied as it is, never converted
            </span>
          </span>
          <button
            type="button"
            onClick={() => toggle(false)}
            disabled={disabled}
            className="ml-auto inline-flex h-9 shrink-0 items-center gap-1.5 rounded-[0.625rem] border border-border-strong px-3 text-[13.5px] font-medium transition-colors outline-none hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:opacity-50"
          >
            <ArrowLeft className="size-4" />
            Back to converting
          </button>
        </div>
      </div>

      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="tracks"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={spring}
            className="overflow-hidden"
          >
            <div className="px-3 pb-3">
              <TracksPanel fileKey={fileKey} version={version} draft={draft} onDraft={onDraft} disabled={disabled} />
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** The layers icon in its tile; its layers spread apart while the tracks are open. */
function LayersTile({ spread }: { spread: boolean }) {
  return (
    <span className="grid size-[2.375rem] shrink-0 place-items-center rounded-[0.7rem] bg-primary-soft text-primary">
      <Layers className={cn("size-[1.15rem] transition-transform duration-500", spread && "scale-y-110")} />
    </span>
  );
}
