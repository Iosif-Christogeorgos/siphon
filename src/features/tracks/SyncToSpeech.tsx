import { AudioLines, Check, Clock3, Lock, LoaderCircle, Undo2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { languageName } from "@/lib/languages";
import { useSubSync } from "@/stores/subsync";
import { useUi } from "@/stores/ui";
import type { SyncProgress } from "@/types";
import {
  type Draft,
  type DraftAdded,
  ownSyncId,
  speechTrack,
  withoutSync,
  withoutTrackSync,
  withSynced,
  withTrackSynced,
} from "./draft";

/** What each stage does, and how much of the whole it takes (as measured on a two-hour film). */
export const STAGES: Record<string, { label: string; from: number; share: number }> = {
  card: { label: "Waiting for room on the graphics card", from: 0, share: 0 },
  subtitles: { label: "Taking its subtitles out of the film", from: 0, share: 0 },
  audio: { label: "Listening to the film", from: 0, share: 0.08 },
  speech: { label: "Finding where people speak", from: 0.08, share: 0.1 },
  words: { label: "Writing down what is said", from: 0.18, share: 0.62 },
  pairing: { label: "Matching each line to its words", from: 0.8, share: 0.2 },
};

export const overall = (progress: SyncProgress | null) => {
  const stage = progress && STAGES[progress.stage];
  return stage ? stage.from + stage.share * progress.fraction : 0;
};

/** Subtitles of a film to sync: a file added to it, or one of its own tracks, by index. */
export type SyncTarget = { added: DraftAdded } | { track: number };

/**
 * Asks for subtitles to be synced to the film's speech (`useSubSync.sync`):
 * now, or once those asked for before are done. The synced copy changes the
 * draft as it is when it comes (`apply`), every choice made of the track
 * kept. Nothing is asked when the film has no sound to listen to.
 */
export function askSync(
  fileKey: string,
  draft: Draft,
  target: SyncTarget,
  apply: (change: (draft: Draft) => Draft) => void,
) {
  const sound = speechTrack(draft);
  if (!sound) return;
  const added = "added" in target ? target.added : null;
  const track = "track" in target ? target.track : null;
  useSubSync.getState().sync(
    {
      id: added ? added.key : ownSyncId(fileKey, track ?? 0),
      file: fileKey,
      subs: added?.key ?? null,
      track,
      stream: sound.index,
      language: sound.language,
      charset: added?.charset ?? null,
    },
    (synced) =>
      apply((current) =>
        added
          ? withSynced(current, added.key, synced.added, synced.summary)
          : withTrackSynced(current, track ?? 0, synced.added, synced.summary),
      ),
  );
}

/**
 * **Sync to speech** for subtitles in text (docs/subtitle-sync.md): an SRT
 * file added to a film, or a text subtitle track the film has. Every line is
 * given the moment its words are spoken. Locked until subtitle sync is
 * downloaded in Settings. The synced copy takes the subtitles' place in the
 * draft, every choice made of their track kept, with a way back.
 */
export function SyncToSpeech({
  fileKey,
  draft,
  target,
  apply,
}: {
  /** The film's key. */
  fileKey: string;
  draft: Draft;
  /** A subtitle file added to the film, or one of the film's own text subtitle tracks, by index. */
  target: SyncTarget;
  /** Changes the draft as it is when the change comes. */
  apply: (change: (draft: Draft) => Draft) => void;
}) {
  const added = "added" in target ? target.added : null;
  const track = "track" in target ? target.track : null;
  const id = added ? added.key : ownSyncId(fileKey, track ?? 0);
  const summary = added ? added.synced?.summary : track !== null ? draft.syncs[track]?.summary : undefined;
  const status = useSubSync((state) => state.status);
  const running = useSubSync((state) => state.running?.id ?? null);
  const progress = useSubSync((state) => state.progress);
  // How many syncs are ahead of this one, while it waits its turn.
  const ahead = useSubSync((state) => {
    const place = state.queue.findIndex((job) => job.id === id);
    return place < 0 ? null : place + (state.running ? 1 : 0);
  });
  const failure = useSubSync((state) => state.failures[id] ?? null);
  const stop = useSubSync((state) => state.stop);
  const openPanel = useUi((state) => state.openPanel);
  if (!status) return null;

  const sound = speechTrack(draft);
  const mine = running === id;
  const undo = () =>
    apply((current) => (added ? withoutSync(current, added.key) : withoutTrackSync(current, track ?? 0)));

  if (summary) {
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-xl bg-primary-soft px-3 py-2 text-xs">
        <Check className="size-3.5 shrink-0 text-primary" strokeWidth={2.5} />
        <span className="min-w-0 flex-1 text-pretty">
          <span className="font-medium text-foreground">Synced to the speech.</span>{" "}
          <span className="text-muted-foreground">
            {summary.by_words} of {summary.lines} lines by their own words
            {summary.placed > 0 && `, ${summary.placed} placed between them`}
            {summary.moved > 0 && `; ${summary.moved} ${summary.moved === 1 ? "sign or song" : "signs and songs"} moved with them`}.
          </span>
        </span>
        <button
          type="button"
          onClick={undo}
          className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-primary transition-colors hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
        >
          <Undo2 className="size-3" />
          {added ? "Use the file’s own timing" : "Use the track’s own timing"}
        </button>
      </div>
    );
  }

  if (ahead !== null) {
    return (
      <div className="flex items-center gap-2 rounded-xl bg-secondary/70 px-3 py-2 text-xs" aria-live="polite">
        <Clock3 className="size-3.5 shrink-0 text-subtle-foreground" />
        <span className="min-w-0 flex-1 truncate text-muted-foreground">
          Waits its turn to sync: {ahead === 1 ? "one file is" : `${ahead} files are`} ahead of it.
        </span>
        <button
          type="button"
          aria-label="Don’t sync it"
          onClick={() => stop([id])}
          className="grid size-6 shrink-0 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      </div>
    );
  }

  if (mine) {
    const share = overall(progress);
    const stage = progress && STAGES[progress.stage];
    const waiting = progress?.stage === "card";
    return (
      <div className="space-y-1.5 rounded-xl bg-secondary/70 px-3 py-2" aria-live="polite">
        <div className="flex items-center gap-2 text-xs">
          <LoaderCircle className="size-3.5 shrink-0 animate-spin text-primary" />
          <span className="min-w-0 flex-1 truncate text-muted-foreground">{stage?.label ?? "Starting"}…</span>
          {!waiting && <span className="shrink-0 font-medium tabular-nums">{Math.round(share * 100)}%</span>}
          <button
            type="button"
            aria-label="Stop syncing"
            onClick={() => stop([id])}
            className="grid size-6 shrink-0 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <X className="size-3.5" />
          </button>
        </div>
        <div className="h-1 overflow-hidden rounded-full bg-background/70">
          <div
            className="h-full rounded-full bg-primary transition-[width] duration-500 ease-out"
            style={{ width: `${Math.max(2, share * 100)}%` }}
          />
        </div>
      </div>
    );
  }

  const ready = status.state === "ready";
  const locked = !ready
    ? status.state === "no_gpu"
      ? "Needs an NVIDIA graphics card with 8 GB of memory."
      : status.state === "installing"
        ? `Subtitle sync is downloading: ${Math.round((status.progress ?? 0) * 100)}%.`
        : null
    : null;
  const busy = running !== null;

  return (
    <div className="space-y-1.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <Button
          size="sm"
          variant={ready ? "default" : "outline"}
          disabled={!ready || !sound}
          onClick={() => askSync(fileKey, draft, target, apply)}
        >
          {ready ? <AudioLines /> : <Lock />}
          Sync to speech
        </Button>
        <span className="min-w-0 flex-1 text-xs text-pretty text-muted-foreground">
          {ready ? (
            !sound ? (
              "The film has no sound to listen to."
            ) : busy ? (
              "Another file is being synced: this one will wait its turn."
            ) : sound.language ? (
              `Gives every line the moment its words are spoken, listening to the ${languageName(sound.language)} sound.`
            ) : (
              "Gives every line the moment its words are spoken in the film."
            )
          ) : (
            (locked ?? (
              <>
                Download subtitle sync in{" "}
                <button
                  type="button"
                  onClick={() => openPanel("settings")}
                  className="rounded-sm text-primary underline-offset-2 hover:underline focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
                >
                  Settings
                </button>{" "}
                to use it.
              </>
            ))
          )}
        </span>
      </div>
      {failure && <p className="text-xs text-pretty text-destructive">{failure}</p>}
    </div>
  );
}
