import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { MessageSquareText, Scissors } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { ChatPanel } from "@/features/chat/ChatPanel";
import { DownloadPanel } from "@/features/download/DownloadPanel";
import { ManualPanel } from "@/features/options/ManualPanel";
import { LiveWaitCard, Note, PlanCard } from "@/features/options/PlanCard";
import { CHAT_GROUPS, PresetArea } from "@/features/presets/PresetArea";
import { changesOf, type Draft, editOf } from "@/features/tracks/draft";
import { useSyncedInUse } from "@/features/tracks/useSyncedInUse";
import { hasTracksToChoose, TracksCard } from "@/features/tracks/TracksCard";
import { TrimPanel } from "@/features/trim/TrimPanel";
import { ipc, toAppError } from "@/lib/ipc";
import { languageName, subtitleLanguages } from "@/lib/languages";
import { spring } from "@/lib/motion";
import { useChoice, useChoices } from "@/stores/choices";
import { isActive, useDownloadGroup, useDownloads } from "@/stores/downloads";
import { useTrim } from "@/stores/trim";
import type { GroupId, PlanSummary, SubtitleDelivery, Trim, VideoInfo } from "@/types";
import { AlreadyDownloaded } from "./AlreadyDownloaded";
import { useMeasuredSize } from "./useMeasuredSize";
import { VideoCard } from "./VideoCard";

/** Several recordings at once are fine; past this many, the sites may start to wonder. */
const MANY_RECORDINGS = 3;

/** Where the plan's subtitles go, after "English subtitles": a track inside the file says nothing more. */
const SUBTITLE_PLACES: Record<SubtitleDelivery, string> = { embed: "", burn: " in the picture" };

/** "No Greek subtitles for this video. It has English and German." */
function missingSubtitles(wanted: string, video: VideoInfo): string {
  const others = subtitleLanguages(video.subtitles).map((language) => language.name);
  const has =
    others.length === 0
      ? "It has none."
      : others.length > 3
        ? `It has ${others.length} other languages.`
        : `It has ${others.slice(0, -1).join(", ")}${others.length > 1 ? " and " : ""}${others.at(-1)}.`;
  return `No ${languageName(wanted)} subtitles for this video. ${has}`;
}

/**
 * What the button says for a file on this PC: what it makes of it. A file
 * that already is what was asked for has nothing to make.
 */
function convertLabel({
  plan,
  group,
  clips,
  clip,
  picture,
}: {
  plan: PlanSummary | undefined;
  /** The preset group chosen, or null while customizing. */
  group: GroupId | null;
  clips: number;
  clip: boolean;
  /** The file has a picture, not only sound. */
  picture: boolean;
}): string {
  if (plan?.nothing) return "Nothing to convert";
  if (plan?.blocked) return "Too long for this limit";
  if (clips > 1) return `Cut ${clips} clips`;
  if (clip) return "Cut the clip";
  if (group === "share") return "Make it fit";
  if (group === "audio" && picture) return "Take the audio";
  if (plan?.badge === "lossless") return "Change the file type";
  return "Convert";
}

/** Everything shown for a loaded video: the card, the trimmer, the choices, the plan and the button. */
export function VideoPane({ video }: { video: VideoInfo }) {
  // A file on this PC, converted rather than downloaded.
  const local = video.file !== undefined;
  // An MKV's tracks can be chosen instead (docs/mkv-editor.md), when there is
  // more to choose than one picture and one sound.
  const overview = video.file?.overview;
  const tracksOffered = hasTracksToChoose(overview, video.file?.editable === true);
  const [tracksOn, setTracksOn] = useState(false);
  // What has been chosen of its tracks so far.
  const [draft, setDraft] = useState<Draft | null>(null);
  useSyncedInUse([draft]);
  const trackChanges = draft ? changesOf(draft) : null;
  const customizing = useChoices((state) => state.choices !== null && state.choices.active.kind !== "preset");
  // A Twitch video's or stream's chat can be drawn into the picture, with the presets that keep it as it is.
  const chatSource = video.chat;
  const chatOffered = useChoices(
    (state) => chatSource !== null && !customizing && CHAT_GROUPS.has(state.choices?.group ?? ""),
  );
  const chatOn = useChoices((state) => state.choices?.chat ?? false) && chatOffered;
  const choice = useChoice(chatOffered && chatSource ? chatSource.channel : null);
  const [placingChat, setPlacingChat] = useState(false);

  // A live stream is recorded: as it airs, or once it starts.
  const live = video.live;
  const onAir = live?.status === "live";
  const waitsForStart = live !== null && !onAir;
  const [fromStart, setFromStart] = useState(false);
  const fromStartOffered = useQuery({
    queryKey: ["from-start", video.url],
    queryFn: () => ipc.liveFromStart(video.url),
    enabled: onAir,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const recordFromStart = fromStart && fromStartOffered.data === true;
  const recordings = useDownloads((state) =>
    state.order.reduce((count, id) => {
      const download = state.byId[id];
      return count + (download?.record && isActive(download) ? 1 : 0);
    }, 0),
  );
  // Share makes a file from the whole video: a stream as it airs gets Max quality instead.
  const group = useChoices((state) => state.choices?.group);
  useEffect(() => {
    if (live && group === "share" && !customizing) useChoices.getState().chooseGroup("max");
  }, [live, group, customizing]);

  const trimOpen = useTrim((state) => state.open);
  const segments = useTrim((state) => state.segments);
  const activeSegment = useTrim((state) => state.activeId);

  const [downloadIds, setDownloadIds] = useState<string[]>([]);
  const download = useDownloadGroup(downloadIds);
  const start = useDownloads((state) => state.start);
  // Choices are locked while a download runs: it was started with the ones shown.
  const running = download !== undefined && download.status !== "finished" && download.status !== "failed";

  // Stream picks and trim ranges belong to the previous video.
  useEffect(() => {
    useChoices.getState().resetForVideo();
    useTrim.getState().reset();
  }, []);

  // A channel whose chat hasn't been placed yet opens the place to put it, once:
  // each streamer's layout differs (a webcam, a minimap).
  const chatPlaced = useChoices((state) =>
    chatSource ? state.choices?.chat_places[chatSource.channel] !== undefined : true,
  );
  const offeredPlacing = useRef(false);
  useEffect(() => {
    if (!chatOn || chatPlaced || offeredPlacing.current) return;
    offeredPlacing.current = true;
    useTrim.getState().close();
    setPlacingChat(true);
  }, [chatOn, chatPlaced]);

  // One download per selected part. Selecting the whole video is no trim at all.
  const trims = useMemo<(Trim | null)[]>(() => {
    if (!trimOpen || !video.duration) return [null];
    const whole = (start: number, end: number) => start <= 0.05 && end >= (video.duration ?? 0) - 0.05;
    const parts = segments
      .filter((segment) => !whole(segment.start, segment.end))
      .map((segment) => ({ start: segment.start, end: segment.end }));
    return parts.length > 0 ? parts : [null];
  }, [trimOpen, segments, video.duration]);

  // The summary describes the part being edited (they all get the same treatment).
  const activeIndex = Math.max(0, segments.findIndex((segment) => segment.id === activeSegment));
  const shownTrim = trims[Math.min(activeIndex, trims.length - 1)] ?? null;

  // Changing a choice after a download finished (or failed) brings back the Download button.
  const choices = JSON.stringify([choice, trims, recordFromStart, tracksOn && draft ? editOf(draft) : null]);
  useEffect(() => {
    setDownloadIds((ids) =>
      ids.some((id) => {
        const download = useDownloads.getState().byId[id];
        return download !== undefined && download.status !== "finished" && download.status !== "failed";
      })
        ? ids
        : [],
    );
  }, [choices]);

  const plan = useQuery({
    // A file's version too: one changed since is planned again.
    queryKey: ["plan", video.url, choice, shownTrim, recordFromStart, video.file?.version ?? null],
    queryFn: () =>
      choice ? ipc.planDownload(video.url, choice, shownTrim, recordFromStart) : Promise.reject(new Error("no choice")),
    // A stream that hasn't started has nothing to plan yet: that happens when it starts.
    enabled: choice !== null && !waitsForStart,
    // Keep showing the last plan while the next one is computed, so the layout doesn't jump.
    placeholderData: keepPreviousData,
  });
  // Where the plan could only estimate the size, it is measured on the video meanwhile.
  const measuring = useMeasuredSize(video.url, choice, shownTrim, plan.data, running);
  // What the plan does with subtitles, and whether the language asked for is missing here.
  const wantedSubtitles = useChoices((state) => state.choices?.subtitles ?? null);
  const subtitleTrack = video.subtitles.find((track) => track.key === plan.data?.subtitle_track);
  const subtitleLabel = subtitleTrack
    ? `${subtitleTrack.name ?? languageName(subtitleTrack.lang)} subtitles${SUBTITLE_PLACES[plan.data?.subtitle_delivery ?? "embed"]}`
    : null;
  const subtitlesMissing =
    !live && wantedSubtitles !== null && plan.data?.subtitle_delivery != null && plan.data.subtitle_track === null;
  // The trimmer and the chat's place each have the preview: one at a time.
  const openTrim = () => {
    setPlacingChat(false);
    useTrim.getState().openFor(video.duration ?? 0);
  };
  const togglePlacingChat = () => {
    if (!placingChat) useTrim.getState().close();
    setPlacingChat(!placingChat);
  };
  const blocked = plan.data?.blocked ?? null;
  const nothing = plan.data?.nothing ?? null;
  const canTrim = video.duration && !trimOpen && !live && !tracksOn;
  const record = live ? { from_start: recordFromStart } : null;

  return (
    <>
      {/* An MKV's tracks are a card tucked behind the file's (docs/mkv-editor.md). */}
      <div className="relative">
        <VideoCard
          video={video}
          compact={tracksOn}
          action={
            chatOn || canTrim ? (
              <div className="flex gap-1.5">
                {chatOn && chatSource && (
                  <Hint label={placingChat ? "Done placing the chat" : "Where the chat goes in the picture"}>
                    <Button
                      variant={placingChat ? "secondary" : "outline"}
                      size="sm"
                      disabled={running}
                      onClick={togglePlacingChat}
                    >
                      <MessageSquareText /> Chat
                    </Button>
                  </Hint>
                )}
                {canTrim && (
                  <Button variant="outline" size="sm" disabled={running} onClick={openTrim}>
                    <Scissors /> Trim
                  </Button>
                )}
              </div>
            ) : null
          }
        />
        {tracksOffered && video.file && overview && (
          <TracksCard
            fileKey={video.file.key}
            version={video.file.version}
            overview={overview}
            open={tracksOn}
            onOpenChange={setTracksOn}
            draft={draft}
            onDraft={setDraft}
            disabled={running}
          />
        )}
      </div>

      {/* A channel's link is the same for every stream it records. */}
      {!live && !local && <AlreadyDownloaded url={video.url} exclude={downloadIds} />}

      <AnimatePresence initial={false}>
        {placingChat && chatOn && chatSource && (
          <motion.div
            key="chat"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={spring}
          >
            <ChatPanel
              video={video}
              channel={chatSource.channel}
              disabled={running}
              onDone={() => setPlacingChat(false)}
            />
          </motion.div>
        )}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {trimOpen && video.duration && !tracksOn && (
          <motion.div
            key="trim"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={spring}
          >
            <TrimPanel
              video={video}
              duration={video.duration}
              disabled={running}
              chatChannel={chatOn && chatSource ? chatSource.channel : null}
            />
          </motion.div>
        )}
      </AnimatePresence>

      {/* Choosing an MKV's tracks replaces converting: its choices and plan fold away, as Trim does. */}
      <AnimatePresence initial={false}>
        {!tracksOn && (
          <motion.div
            key="convert"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto", filter: "blur(0px)" }}
            exit={{ opacity: 0, height: 0, filter: "blur(3px)" }}
            transition={spring}
            // Its parts keep the pane's spacing between them.
            className="space-y-3"
          >
            {/* Extra room below: the choices end here and the summary of the result begins. */}
            <div className="space-y-2 pt-1 pb-2">
              <PresetArea
                summary={plan.data}
                disabled={running}
                live={live !== null}
                from={fromStartOffered.data ? { start: fromStart, onChange: setFromStart } : undefined}
                subtitles={live ? undefined : video.subtitles}
                chat={chatSource !== null}
                file={local}
              />
              <AnimatePresence initial={false}>
                {customizing && (
                  <motion.div
                    key="custom"
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: "auto" }}
                    exit={{ opacity: 0, height: 0 }}
                    transition={spring}
                  >
                    <ManualPanel video={video} disabled={running} />
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
            {live && live.status !== "live" ? (
              <LiveWaitCard status={live.status} />
            ) : plan.isError ? (
              <ErrorCard error={toAppError(plan.error)} />
            ) : (
              <PlanCard
                plan={plan.data}
                clips={trims.length}
                customizing={customizing}
                onCustomize={() =>
                  customizing ? useChoices.getState().chooseGroup(useChoices.getState().choices?.group ?? "") : useChoices.getState().customize()
                }
                customizable={!local}
                onTrim={canTrim ? openTrim : undefined}
                perHour={live !== null}
                subtitles={subtitleLabel}
                measuring={measuring}
              />
            )}
          </motion.div>
        )}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {subtitlesMissing && !tracksOn && (
          <Note key="no-subtitles" warning={false}>
            {missingSubtitles(wantedSubtitles, video)}
          </Note>
        )}
      </AnimatePresence>

      <AnimatePresence initial={false}>
        {live && !running && recordings >= MANY_RECORDINGS && (
          <Note key="many" warning={false}>
            {recordings} recordings are running already. Each more makes sites likelier to ask whether you’re a
            bot, and each needs its share of your connection.
          </Note>
        )}
      </AnimatePresence>

      {tracksOn ? (
        <DownloadPanel
          download={download}
          onStart={() => {
            if (!choice || !draft || !video.file) return;
            // A file is listed by its name, as in a list of files.
            const source = { ...video, title: video.file.name };
            setDownloadIds([start(source, choice, null, null, null, editOf(draft))]);
          }}
          disabled={!choice || !trackChanges?.changed || trackChanges.kept + trackChanges.added === 0}
          convert
          nothing={trackChanges !== null && !trackChanges.changed}
          label={
            !trackChanges || !trackChanges.changed
              ? "Nothing to change"
              : trackChanges.kept + trackChanges.added === 0
                ? "Keep at least one track"
                : trackChanges.removed > 0 && trackChanges.added === 0
                  ? `Save without ${trackChanges.removed} track${trackChanges.removed === 1 ? "" : "s"}`
                  : trackChanges.added > 0 && trackChanges.removed === 0
                    ? `Save with ${trackChanges.added} more track${trackChanges.added === 1 ? "" : "s"}`
                    : "Save the changes"
          }
        />
      ) : (
        <DownloadPanel
          download={download}
          onStart={() => {
            if (!choice) return;
            // A file is listed by its name, as in a list of files.
            const source = video.file ? { ...video, title: video.file.name } : video;
            setDownloadIds(trims.map((trim) => start(source, choice, trim, null, record)));
          }}
          disabled={waitsForStart ? choice === null : !plan.isSuccess || blocked !== null || nothing !== null}
          record={live !== null}
          convert={local}
          nothing={nothing !== null}
          label={
            local
              ? convertLabel({
                  plan: plan.data,
                  group: customizing ? null : (group ?? null),
                  clips: trims.length,
                  clip: shownTrim !== null,
                  picture: video.formats[0]?.kind !== "audio",
                })
              : waitsForStart
              ? "Record when it starts"
              : live
                ? "Record"
                : blocked
                  ? "Too long for this limit"
                  : trims.length > 1
                    ? `Download ${trims.length} clips`
                    : shownTrim
                      ? "Download clip"
                      : "Download"
          }
        />
      )}
    </>
  );
}
