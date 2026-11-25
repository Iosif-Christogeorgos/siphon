import { Captions, Check, Minus, Plus, TriangleAlert, Volume2, X, Zap } from "lucide-react";
import { AnimatePresence } from "motion/react";
import { useRef, useState } from "react";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Note } from "@/features/options/PlanCard";
import { formatBytes } from "@/lib/format";
import { ipc, toAppError } from "@/lib/ipc";
import { languageName } from "@/lib/languages";
import { cn } from "@/lib/utils";
import { useUi } from "@/stores/ui";
import type { PickedFile } from "@/types";
import { addTracks, type Draft } from "./draft";
import {
  allAsItIs,
  allMine,
  firstRow,
  keepRow,
  type Role,
  rowsOf,
  type SeasonFile,
  type SeasonRow,
  seasonSummary,
  toCheck,
} from "./season";

const ROLE_LABELS: Record<Role, string | null> = {
  main: null,
  commentary: "Commentary",
  described: "Described",
  forced: "Forced",
  sdh: "SDH",
};

const KINDS = [
  { kind: "sound", label: "Sound", icon: Volume2 },
  { kind: "subtitles", label: "Subtitles", icon: Captions },
] as const;

/**
 * A season's tracks, by language and kind across its files: keeping or
 * leaving out "English commentary", or making Greek subtitles play first,
 * does so in every file that has them (see `season`). Each file can still be
 * opened in the list below and changed by hand.
 */
export function SeasonTracks({
  files,
  total,
  reading,
  onDrafts,
  disabled,
}: {
  /** The files read so far, each with its draft. */
  files: SeasonFile[];
  /** How many of the list's files have tracks to choose. */
  total: number;
  /** How many are still being read. */
  reading: number;
  onDrafts: (drafts: Map<string, Draft>) => void;
  disabled: boolean;
}) {
  const openPanel = useUi((state) => state.openPanel);
  if (files.length === 0) {
    return (
      <div aria-busy className="space-y-2 px-1 py-1.5">
        <p className="text-xs text-muted-foreground tabular-nums">
          Reading their tracks and subtitles… {total - reading} of {total}
        </p>
        {[0, 1, 2].map((row) => (
          <div key={row} aria-hidden className="skeleton h-9 rounded-xl" />
        ))}
      </div>
    );
  }

  const rows = rowsOf(files);
  const mine = files.every((file) => file.draft.mine);
  const languages = files.find((file) => file.contents.suggestion)?.contents.suggestion?.languages ?? [];
  const names = languages.map((language) => languageName(language)).join(", ") || "none chosen";
  const leftOutByLanguages = mine
    ? files.reduce((sum, file) => sum + (file.contents.suggestion?.left_out.length ?? 0), 0)
    : 0;
  const unsure = files.filter((file) => toCheck(file.contents, file.draft).length > 0).length;
  const { copies, removed, added, size } = seasonSummary(files);

  return (
    <fieldset disabled={disabled} className="min-w-0 space-y-3 border-0 p-0 py-1.5">
      <div className="flex items-start gap-3 rounded-xl bg-secondary px-3 py-2.5 text-[13px] text-muted-foreground">
        <p className="min-w-0 flex-1 text-pretty">
          <span className="text-foreground">
            {mine ? `Kept for your languages: ${names}.` : "Kept as the files have them."}
          </span>{" "}
          {mine && leftOutByLanguages > 0 && `They leave out ${leftOutByLanguages} tracks in all. `}
          Choices here are made in every file. Your languages are in{" "}
          <button
            type="button"
            onClick={() => openPanel("settings")}
            className="rounded-sm text-primary underline-offset-2 hover:underline focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
          >
            Settings
          </button>
          .
        </p>
        <button
          type="button"
          onClick={() => onDrafts(mine ? allAsItIs(files) : allMine(files))}
          className="-my-1 h-7 shrink-0 rounded-lg bg-background/70 px-2.5 text-xs font-medium text-foreground transition-colors hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
        >
          {mine ? "Keep everything" : "Keep my languages"}
        </button>
      </div>

      {KINDS.map(({ kind, label, icon: Icon }) => {
        const ofKind = rows.filter((row) => row.kind === kind);
        if (ofKind.length === 0) return null;
        return (
          <section key={kind} aria-label={label}>
            <p className="px-1 pb-1 text-xs font-medium text-subtle-foreground">{label}</p>
            <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
              {ofKind.map((row) => (
                <Row
                  key={row.key}
                  row={row}
                  icon={<Icon className="size-4 shrink-0 text-muted-foreground" />}
                  total={total}
                  onKeep={(keep) => onDrafts(keepRow(files, row.key, keep))}
                  onFirst={(on) => onDrafts(firstRow(files, row.key, on))}
                />
              ))}
            </ul>
          </section>
        );
      })}

      <SeasonAdd files={files} onDrafts={onDrafts} />

      {unsure > 0 && (
        <Note warning>
          {unsure === 1 ? "One file has" : `${unsure} files have`} tracks whose language Siphon isn’t sure of. They
          are kept, whatever is chosen here: open {unsure === 1 ? "the file" : "those files"} in the list below to see
          why, and choose by hand.
        </Note>
      )}
      {reading > 0 && (
        <p className="px-1 text-xs text-muted-foreground tabular-nums">
          Still reading {reading} {reading === 1 ? "file" : "files"}: choices made now are made in them too, once read.
        </p>
      )}

      <div className="flex items-center gap-3 px-1 pt-1">
        <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary-soft text-primary">
          <Zap className="size-4" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">
            {copies === 0
              ? "Keeps them as they are"
              : `${copies} ${copies === 1 ? "copy" : "copies"}${removed > 0 ? `, ${removed} ${removed === 1 ? "track" : "tracks"} left out in all` : ""}${added > 0 ? `, ${added} added` : ""}`}
          </p>
          <p className="text-xs text-muted-foreground">
            <span className="text-primary">No quality loss</span> · Every track copied as it is
          </p>
        </div>
        {copies > 0 && size !== null && (
          <span className="shrink-0 text-[15px] font-medium tabular-nums">
            <span className="text-muted-foreground">≈ </span>
            {formatBytes(size)}
          </span>
        )}
      </div>
    </fieldset>
  );
}

function Row({
  row,
  icon,
  total,
  onKeep,
  onFirst,
}: {
  row: SeasonRow;
  icon: React.ReactNode;
  total: number;
  onKeep: (keep: boolean) => void;
  onFirst: (on: boolean) => void;
}) {
  const role = ROLE_LABELS[row.role];
  const name = languageName(row.language);
  const kept = row.keep !== "none";
  return (
    <li className={cn("flex items-center gap-3 px-3 py-2 transition-opacity", !kept && "opacity-55")}>
      <button
        type="button"
        role="checkbox"
        aria-checked={row.keep === "all" ? true : row.keep === "none" ? false : "mixed"}
        aria-label={`Keep ${name}${role ? ` ${role.toLowerCase()}` : ""} ${row.kind}`}
        onClick={() => onKeep(row.keep !== "all")}
        className={cn(
          "grid size-5 shrink-0 place-items-center rounded-md border transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
          kept
            ? "border-primary bg-primary text-primary-foreground"
            : "border-border-strong bg-card text-transparent hover:border-primary",
        )}
      >
        {row.keep === "some" ? <Minus className="size-3.5" strokeWidth={3} /> : <Check className="size-3.5" strokeWidth={3} />}
      </button>
      {icon}
      <div className="min-w-0 flex-1">
        <p className={cn("truncate text-sm font-medium", !kept && "line-through")}>
          {name}
          {role && <span className="font-normal text-muted-foreground"> · {role}</span>}
          {row.added && <span className="font-normal text-muted-foreground"> · Added</span>}
        </p>
        <p className="truncate text-xs text-muted-foreground tabular-nums">
          {row.files === total ? `In every file (${total})` : `In ${row.files} of ${total} files`}
          {row.keep === "some" && " · kept in some"}
        </p>
      </div>
      {kept && (
        <button
          type="button"
          aria-pressed={row.first === "all"}
          onClick={() => onFirst(row.first !== "all")}
          className={cn(
            "h-7 shrink-0 rounded-lg px-2.5 text-xs transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
            row.first === "all"
              ? "bg-primary-soft text-primary"
              : row.first === "some"
                ? "text-primary hover:bg-accent"
                : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
        >
          {row.first === "some" ? "Plays first in some" : "Plays first"}
        </button>
      )}
    </li>
  );
}

/**
 * Adding tracks to a season's episodes: each file picked goes with the
 * episode its name matches ("Show S01E03.el.srt" with "Show S01E03.mkv", or
 * the same season and episode); one that matches none is listed, to be given
 * an episode by hand.
 */
function SeasonAdd({ files, onDrafts }: { files: SeasonFile[]; onDrafts: (drafts: Map<string, Draft>) => void }) {
  // The files are read a moment after they are picked: their tracks go in the drafts as they are then.
  const latest = useRef(files);
  latest.current = files;
  const [reading, setReading] = useState(0);
  const [failed, setFailed] = useState<string[]>([]);
  const [unplaced, setUnplaced] = useState<PickedFile[]>([]);

  const addTo = async (pairs: { file: PickedFile; episode: string }[]) => {
    if (pairs.length === 0) return;
    setReading((count) => count + pairs.length);
    const reads = await Promise.allSettled(pairs.map((pair) => ipc.readAdded(pair.file.key)));
    setReading((count) => count - pairs.length);
    setFailed(reads.flatMap((read) => (read.status === "rejected" ? [toAppError(read.reason).message] : [])));
    const drafts = new Map<string, Draft>();
    reads.forEach((read, at) => {
      const episode = pairs[at]?.episode;
      if (read.status !== "fulfilled" || !episode) return;
      const draft = drafts.get(episode) ?? latest.current.find((file) => file.key === episode)?.draft;
      if (draft) drafts.set(episode, addTracks(draft, read.value));
    });
    if (drafts.size > 0) onDrafts(drafts);
  };

  const add = async () => {
    const picked = await ipc.pickTrackFiles().catch(() => null);
    if (!picked || picked.length === 0) return;
    const episodes = latest.current;
    const matches = await ipc.matchEpisodes(
      episodes.map((file) => file.name),
      picked.map((file) => file.name),
    );
    const pairs = picked.flatMap((file, at) => {
      const place = matches[at];
      const episode = place != null ? episodes[place] : undefined;
      return episode ? [{ file, episode: episode.key }] : [];
    });
    setUnplaced((now) => [
      ...now,
      ...picked.filter((file, at) => matches[at] == null && !now.some((other) => other.key === file.key)),
    ]);
    await addTo(pairs);
  };

  return (
    <div className="space-y-2">
      <button
        type="button"
        onClick={() => void add()}
        disabled={reading > 0}
        className="flex h-12 w-full items-center gap-2.5 rounded-xl border-[1.5px] border-dashed border-border-strong px-3.5 text-left text-[13.5px] text-muted-foreground transition-colors outline-none hover:border-primary/50 hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:cursor-progress"
      >
        <Plus className="size-4 shrink-0" />
        <span className="min-w-0 flex-1 truncate">
          {reading > 0 ? `Reading ${reading} ${reading === 1 ? "file" : "files"}…` : "Add subtitles or sound to the episodes"}
        </span>
        <span className="shrink-0 text-xs text-subtle-foreground">Matched by name</span>
      </button>
      {unplaced.map((file) => (
        <div key={file.key} className="flex items-center gap-2.5 rounded-xl bg-secondary px-3 py-1.5 text-[13px]">
          <TriangleAlert className="size-3.5 shrink-0 text-destructive" />
          <span className="min-w-0 flex-1 truncate" title={file.name}>
            {file.name} <span className="text-muted-foreground">matches no episode by its name</span>
          </span>
          <Select
            onValueChange={(episode) => {
              setUnplaced((now) => now.filter((other) => other.key !== file.key));
              void addTo([{ file, episode }]);
            }}
          >
            <SelectTrigger aria-label={`Episode for ${file.name}`} className="h-8 w-44">
              <SelectValue placeholder="Add it to…" />
            </SelectTrigger>
            <SelectContent className="max-h-72">
              {files.map((episode) => (
                <SelectItem key={episode.key} value={episode.key}>
                  {episode.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <button
            type="button"
            aria-label={`Don’t add ${file.name}`}
            onClick={() => setUnplaced((now) => now.filter((other) => other.key !== file.key))}
            className="grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <X className="size-3.5" />
          </button>
        </div>
      ))}
      <AnimatePresence initial={false}>
        {failed.map((why) => (
          <Note key={why} warning>
            {why}
          </Note>
        ))}
      </AnimatePresence>
    </div>
  );
}

/** The mark on a file whose tracks aren't all certain, with why on hover. */
export function UnsureMark({ lines }: { lines: string[] }) {
  return (
    <span className="flex shrink-0 items-center gap-1 rounded-md bg-destructive-soft px-1.5 py-0.5 text-[11px] font-medium text-foreground">
      <TriangleAlert className="size-3 text-destructive" />
      {lines.length === 1 ? "1 to check" : `${lines.length} to check`}
    </span>
  );
}
