import { AudioLines, Check, Lock, LoaderCircle, Undo2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { languageName } from "@/lib/languages";
import { useSubSync } from "@/stores/subsync";
import { useUi } from "@/stores/ui";
import type { SyncProgress } from "@/types";
import { type Draft, type DraftAdded, speechTrack, withoutSync, withSynced } from "./draft";

/** What each stage does, and how much of the whole it takes (as measured on a two-hour film). */
const STAGES: Record<string, { label: string; from: number; share: number }> = {
  audio: { label: "Listening to the film", from: 0, share: 0.08 },
  speech: { label: "Finding where people speak", from: 0.08, share: 0.1 },
  words: { label: "Writing down what is said", from: 0.18, share: 0.62 },
  pairing: { label: "Matching each line to its words", from: 0.8, share: 0.2 },
};

const overall = (progress: SyncProgress | null) => {
  const stage = progress && STAGES[progress.stage];
  return stage ? stage.from + stage.share * progress.fraction : 0;
};

/**
 * **Sync to speech** for a subtitle file added to a film (docs/subtitle-sync.md):
 * every line given the moment its words are spoken. Locked until subtitle
 * sync is downloaded in Settings. The synced copy takes the file's place in
 * the draft, every choice made of its track kept, with a way back.
 */
export function SyncToSpeech({
  fileKey,
  draft,
  added,
  apply,
}: {
  /** The film's key. */
  fileKey: string;
  draft: Draft;
  added: DraftAdded;
  /** Changes the draft as it is when the change comes. */
  apply: (change: (draft: Draft) => Draft) => void;
}) {
  const status = useSubSync((state) => state.status);
  const running = useSubSync((state) => state.running);
  const progress = useSubSync((state) => state.progress);
  const failure = useSubSync((state) => (state.failure?.key === added.key ? state.failure.message : null));
  const sync = useSubSync((state) => state.sync);
  const cancel = useSubSync((state) => state.cancel);
  const openPanel = useUi((state) => state.openPanel);
  if (!status) return null;

  const sound = speechTrack(draft);
  const mine = running === added.key;

  if (added.synced) {
    const { summary } = added.synced;
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-xl bg-primary-soft px-3 py-2 text-xs">
        <Check className="size-3.5 shrink-0 text-primary" strokeWidth={2.5} />
        <span className="min-w-0 flex-1 text-pretty">
          <span className="font-medium text-foreground">Synced to the speech.</span>{" "}
          <span className="text-muted-foreground">
            {summary.by_words} of {summary.lines} lines by their own words
            {summary.placed > 0 && `, ${summary.placed} placed between them`}.
          </span>
        </span>
        <button
          type="button"
          onClick={() => apply((current) => withoutSync(current, added.key))}
          className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-primary transition-colors hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
        >
          <Undo2 className="size-3" />
          Use the file’s own timing
        </button>
      </div>
    );
  }

  if (mine) {
    const share = overall(progress);
    const stage = progress && STAGES[progress.stage];
    return (
      <div className="space-y-1.5 rounded-xl bg-secondary/70 px-3 py-2" aria-live="polite">
        <div className="flex items-center gap-2 text-xs">
          <LoaderCircle className="size-3.5 shrink-0 animate-spin text-primary" />
          <span className="min-w-0 flex-1 truncate text-muted-foreground">{stage?.label ?? "Starting"}…</span>
          <span className="shrink-0 font-medium tabular-nums">{Math.round(share * 100)}%</span>
          <button
            type="button"
            aria-label="Stop syncing"
            onClick={cancel}
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
          disabled={!ready || busy || !sound}
          onClick={() =>
            sound &&
            void sync(
              {
                file: fileKey,
                subs: added.key,
                stream: sound.index,
                language: sound.language,
                charset: added.charset,
              },
              (synced) => apply((current) => withSynced(current, added.key, synced.added, synced.summary)),
            )
          }
        >
          {ready ? <AudioLines /> : <Lock />}
          Sync to speech
        </Button>
        <span className="min-w-0 flex-1 text-xs text-pretty text-muted-foreground">
          {ready ? (
            busy ? (
              "Another file is being synced."
            ) : !sound ? (
              "The film has no sound to listen to."
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
