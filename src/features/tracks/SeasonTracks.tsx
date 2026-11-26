import { BookOpen, Captions, Check, Minus, Paperclip, Plus, TriangleAlert, Volume2, X, Zap } from "lucide-react";
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
import { type Draft, withFonts } from "./draft";
import { FlagToggle } from "./FlagToggle";
import { kindOfPicked, readPicked } from "./reading";
import { applyPicked } from "./TracksPanel";
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

      <SeasonExtras files={files} onDrafts={onDrafts} />

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
        <FlagToggle
          on={row.first === "all" ? true : row.first === "some" ? "some" : false}
          onClick={() => onFirst(row.first !== "all")}
          className="h-7"
        >
          {row.first === "some" ? "Plays first in some" : "Plays first"}
        </FlagToggle>
      )}
    </li>
  );
}

/**
 * Chapters and attached files, across the season: kept in every file, or
 * left out of every file; fonts added to every file, or taken away.
 */
function SeasonExtras({ files, onDrafts }: { files: SeasonFile[]; onDrafts: (drafts: Map<string, Draft>) => void }) {
  const withChapters = files.filter((file) => file.contents.chapters.count > 0 || file.draft.chapterList !== null);
  const withAttachments = files.filter((file) => file.contents.attachments.length > 0);
  const fonts = files[0]?.draft.fonts.filter((font) => files.every((file) => file.draft.fonts.some((f) => f.key === font.key))) ?? [];
  if (withChapters.length === 0 && withAttachments.length === 0 && fonts.length === 0) return null;
  const state = (count: number, of: number) => (count === 0 ? "none" : count === of ? "all" : "some");
  const chaptersKept = state(
    withChapters.filter((file) => file.draft.chapters || file.draft.chapterList !== null).length,
    withChapters.length,
  );
  const attachmentsKept = state(
    withAttachments.filter((file) => file.draft.attachments && file.draft.leftOutAttachments.length === 0).length,
    withAttachments.length,
  );
  const each = (of: SeasonFile[], change: (draft: Draft, file: SeasonFile) => Draft) =>
    onDrafts(new Map(of.map((file) => [file.key, change(file.draft, file)])));
  return (
    <section aria-label="Chapters and attached files">
      <p className="px-1 pb-1 text-xs font-medium text-subtle-foreground">Chapters and attached files</p>
      <ul className="divide-y divide-border/60 overflow-hidden rounded-xl bg-secondary/50">
        {withChapters.length > 0 && (
          <ExtraRow
            icon={<BookOpen className="size-4 shrink-0 text-muted-foreground" />}
            name="Chapters"
            said={withChapters.length === files.length ? `In every file (${files.length})` : `In ${withChapters.length} of ${files.length} files`}
            kept={chaptersKept}
            onKeep={(keep) =>
              each(withChapters, (draft) =>
                keep ? { ...draft, chapters: true } : { ...draft, chapters: false, chapterList: null, chaptersFrom: null },
              )
            }
          />
        )}
        {withAttachments.length > 0 && (
          <ExtraRow
            icon={<Paperclip className="size-4 shrink-0 text-muted-foreground" />}
            name="Their attached files"
            said="Fonts for ASS subtitles, covers"
            kept={attachmentsKept}
            onKeep={(keep) =>
              each(withAttachments, (draft, file) => ({
                ...draft,
                attachments: true,
                leftOutAttachments: keep ? [] : file.contents.attachments.map((attachment) => attachment.index),
              }))
            }
          />
        )}
        {fonts.map((font) => (
          <li key={font.key} className="flex items-center gap-3 px-3 py-2">
            <Paperclip className="size-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium">
                {font.name}
                <span className="font-normal text-muted-foreground"> · Added</span>
              </p>
              <p className="truncate text-xs text-muted-foreground">Attached to every file ({files.length})</p>
            </div>
            <button
              type="button"
              aria-label={`Don’t attach ${font.name}`}
              onClick={() => each(files, (draft) => ({ ...draft, fonts: draft.fonts.filter((f) => f.key !== font.key) }))}
              className="grid size-7 shrink-0 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
            >
              <X className="size-3.5" />
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function ExtraRow({
  icon,
  name,
  said,
  kept,
  onKeep,
}: {
  icon: React.ReactNode;
  name: string;
  said: string;
  kept: "all" | "none" | "some";
  onKeep: (keep: boolean) => void;
}) {
  return (
    <li className={cn("flex items-center gap-3 px-3 py-2 transition-opacity", kept === "none" && "opacity-55")}>
      <button
        type="button"
        role="checkbox"
        aria-checked={kept === "all" ? true : kept === "none" ? false : "mixed"}
        aria-label={`Keep ${name.toLowerCase()}`}
        onClick={() => onKeep(kept !== "all")}
        className={cn(
          "grid size-5 shrink-0 place-items-center rounded-md border transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
          kept !== "none"
            ? "border-primary bg-primary text-primary-foreground"
            : "border-border-strong bg-card text-transparent hover:border-primary",
        )}
      >
        {kept === "some" ? <Minus className="size-3.5" strokeWidth={3} /> : <Check className="size-3.5" strokeWidth={3} />}
      </button>
      {icon}
      <div className="min-w-0 flex-1">
        <p className={cn("truncate text-sm font-medium", kept === "none" && "line-through")}>{name}</p>
        <p className="truncate text-xs text-muted-foreground">
          {said}
          {kept === "some" && " · kept in some"}
        </p>
      </div>
    </li>
  );
}

/**
 * Adding to a season's episodes: each subtitle, sound or chapters file
 * picked goes with the episode its name matches ("Show S01E03.el.srt" with
 * "Show S01E03.mkv", or the same season and episode); one that matches none
 * is listed, to be given an episode by hand. A font goes in every episode.
 */
function SeasonAdd({ files, onDrafts }: { files: SeasonFile[]; onDrafts: (drafts: Map<string, Draft>) => void }) {
  // The files are read a moment after they are picked: they go in the drafts as they are then.
  const latest = useRef(files);
  latest.current = files;
  const [reading, setReading] = useState(0);
  const [failed, setFailed] = useState<string[]>([]);
  const [unplaced, setUnplaced] = useState<PickedFile[]>([]);

  const addTo = async (pairs: { file: PickedFile; episode: string }[]) => {
    if (pairs.length === 0) return;
    setReading((count) => count + pairs.length);
    const reads = await Promise.allSettled(pairs.map((pair) => readPicked(pair.file)));
    setReading((count) => count - pairs.length);
    setFailed(reads.flatMap((read) => (read.status === "rejected" ? [toAppError(read.reason).message] : [])));
    const drafts = new Map<string, Draft>();
    reads.forEach((read, at) => {
      const episode = pairs[at]?.episode;
      if (read.status !== "fulfilled" || !episode) return;
      const draft = drafts.get(episode) ?? latest.current.find((file) => file.key === episode)?.draft;
      if (draft) drafts.set(episode, applyPicked(draft, read.value));
    });
    if (drafts.size > 0) onDrafts(drafts);
  };

  const add = async () => {
    const picked = await ipc.pickTrackFiles().catch(() => null);
    if (!picked || picked.length === 0) return;
    const episodes = latest.current;
    // Fonts are for every episode: their ASS subtitles are drawn with them.
    const fonts = picked.filter((file) => kindOfPicked(file.name) === "font");
    if (fonts.length > 0) onDrafts(new Map(episodes.map((file) => [file.key, withFonts(file.draft, fonts)])));
    const rest = picked.filter((file) => kindOfPicked(file.name) !== "font");
    if (rest.length === 0) return;
    const matches = await ipc.matchEpisodes(
      episodes.map((file) => file.name),
      rest.map((file) => file.name),
    );
    const pairs = rest.flatMap((file, at) => {
      const place = matches[at];
      const episode = place != null ? episodes[place] : undefined;
      return episode ? [{ file, episode: episode.key }] : [];
    });
    setUnplaced((now) => [
      ...now,
      ...rest.filter((file, at) => matches[at] == null && !now.some((other) => other.key === file.key)),
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
          {reading > 0
            ? `Reading ${reading} ${reading === 1 ? "file" : "files"}…`
            : "Add subtitles, sound, chapters or fonts to the episodes"}
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
