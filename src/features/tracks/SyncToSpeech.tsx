import { languageName } from "@/lib/languages";
import { useSubSync } from "@/stores/subsync";
import { useUi } from "@/stores/ui";
import type { SyncProgress } from "@/types";
import {
  type Draft,
  type DraftAdded,
  addedSyncId,
  ownSyncId,
  speechTrack,
  withoutSync,
  withoutTrackSync,
  withSynced,
  withTrackSynced,
} from "./draft";
import { SyncKey } from "./SyncKey";

/** What each stage does, and how much of the whole it takes (as measured on a two-hour film). */
export const STAGES: Record<string, { label: string; short: string; from: number; share: number }> = {
  card: { label: "Waiting for room on the graphics card", short: "Waiting for room", from: 0, share: 0 },
  subtitles: { label: "Taking its subtitles out of the film", short: "Taking them out", from: 0, share: 0 },
  audio: { label: "Listening to the film", short: "Listening", from: 0, share: 0.08 },
  speech: { label: "Finding where people speak", short: "Finding speech", from: 0.08, share: 0.1 },
  words: { label: "Writing down what is said", short: "Writing it down", from: 0.18, share: 0.62 },
  pairing: { label: "Matching each line to its words", short: "Matching lines", from: 0.8, share: 0.2 },
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
      id: added ? addedSyncId(fileKey, added.key) : ownSyncId(fileKey, track ?? 0),
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
 * given the moment its words are spoken. A key beside the track's flags
 * (`SyncKey`), locked until subtitle sync is downloaded in Settings. The synced
 * copy takes the subtitles' place in the draft, every choice made of their
 * track kept, with a way back.
 */
export function SyncToSpeech({
  fileKey,
  draft,
  target,
  apply,
  className,
}: {
  /** The film's key. */
  fileKey: string;
  draft: Draft;
  /** A subtitle file added to the film, or one of the film's own text subtitle tracks, by index. */
  target: SyncTarget;
  /** Changes the draft as it is when the change comes. */
  apply: (change: (draft: Draft) => Draft) => void;
  /** Where the key sits in the row of flags. */
  className?: string;
}) {
  const added = "added" in target ? target.added : null;
  const track = "track" in target ? target.track : null;
  const id = added ? addedSyncId(fileKey, added.key) : ownSyncId(fileKey, track ?? 0);
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

  const own = added ? "Use the file’s own timing" : "Use the track’s own timing";

  if (summary) {
    return (
      <SyncKey
        state="done"
        label="Synced"
        className={className}
        hint={
          <>
            Synced to the speech: {summary.by_words} of {summary.lines} lines by their own words
            {summary.placed > 0 && `, ${summary.placed} placed between them`}
            {summary.moved > 0 && `; ${summary.moved} ${summary.moved === 1 ? "sign or song" : "signs and songs"} moved with them`}.
          </>
        }
        side={{ label: own, icon: "undo", onClick: undo }}
      />
    );
  }

  if (ahead !== null) {
    return (
      <SyncKey
        state="waiting"
        label="Waits its turn"
        aside={`${ahead} ahead`}
        className={className}
        hint={`Waits its turn to sync: ${ahead === 1 ? "one file is" : `${ahead} files are`} ahead of it.`}
        side={{ label: "Don’t sync it", icon: "stop", onClick: () => stop([id]) }}
      />
    );
  }

  if (mine) {
    const share = overall(progress);
    const stage = progress && STAGES[progress.stage];
    return (
      <SyncKey
        state="running"
        label={stage?.short ?? "Starting"}
        // Waiting for room on the card has nothing to count.
        aside={progress?.stage === "card" ? null : `${Math.round(share * 100)}%`}
        share={share}
        className={className}
        hint={`${stage?.label ?? "Starting"}…`}
        side={{ label: "Stop syncing", icon: "stop", onClick: () => stop([id]) }}
      />
    );
  }

  const ready = status.state === "ready";
  const locked =
    status.state === "no_gpu"
      ? "Needs an NVIDIA graphics card with 8 GB of memory."
      : status.state === "installing"
        ? `Subtitle sync is downloading: ${Math.round((status.progress ?? 0) * 100)}%.`
        : null;

  return (
    <>
      {ready ? (
        <SyncKey
          state="idle"
          label="Sync to speech"
          disabled={!sound}
          className={className}
          hint={
            !sound
              ? "The film has no sound to listen to."
              : running !== null
                ? "Another file is being synced: this one will wait its turn."
                : sound.language
                  ? `Gives every line the moment its words are spoken, listening to the ${languageName(sound.language)} sound.`
                  : "Gives every line the moment its words are spoken in the film."
          }
          onPress={() => askSync(fileKey, draft, target, apply)}
        />
      ) : (
        <SyncKey
          state="locked"
          label="Sync to speech"
          disabled={locked !== null}
          className={className}
          hint={locked ?? "Download subtitle sync in Settings to use it."}
          onPress={() => openPanel("settings")}
        />
      )}
      {/* Under the whole row of flags, whatever comes after the key in it. */}
      {failure && <p className="order-last basis-full text-xs text-pretty text-destructive">{failure}</p>}
    </>
  );
}
