import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Scissors } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";
import { DownloadPanel } from "@/features/download/DownloadPanel";
import { ManualPanel } from "@/features/options/ManualPanel";
import { PlanCard } from "@/features/options/PlanCard";
import { PresetArea } from "@/features/presets/PresetArea";
import { TrimPanel } from "@/features/trim/TrimPanel";
import { ipc, toAppError } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { useChoice, useChoices } from "@/stores/choices";
import { useDownloadGroup, useDownloads } from "@/stores/downloads";
import { useTrim } from "@/stores/trim";
import type { Trim, VideoInfo } from "@/types";
import { VideoCard } from "./VideoCard";

/** Everything shown for a loaded video: the card, the trimmer, the choices, the plan and the button. */
export function VideoPane({ video }: { video: VideoInfo }) {
  const choice = useChoice();
  const customizing = useChoices((state) => state.choices !== null && state.choices.active.kind !== "preset");

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
  const choices = JSON.stringify([choice, trims]);
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
    queryKey: ["plan", video.url, choice, shownTrim],
    queryFn: () => (choice ? ipc.planDownload(video.url, choice, shownTrim) : Promise.reject(new Error("no choice"))),
    enabled: choice !== null,
    // Keep showing the last plan while the next one is computed, so the layout doesn't jump.
    placeholderData: keepPreviousData,
  });
  const openTrim = () => useTrim.getState().openFor(video.duration ?? 0);
  const blocked = plan.data?.blocked ?? null;

  return (
    <>
      <VideoCard
        video={video}
        action={
          video.duration && !trimOpen ? (
            <Button variant="outline" size="sm" disabled={running} onClick={openTrim}>
              <Scissors /> Trim
            </Button>
          ) : null
        }
      />

      <AnimatePresence initial={false}>
        {trimOpen && video.duration && (
          <motion.div
            key="trim"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={spring}
          >
            <TrimPanel video={video} duration={video.duration} disabled={running} />
          </motion.div>
        )}
      </AnimatePresence>

      <div className="space-y-2 pt-1">
        <PresetArea summary={plan.data} disabled={running} />
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

      {plan.isError ? (
        <ErrorCard error={toAppError(plan.error)} />
      ) : (
        <PlanCard
          plan={plan.data}
          clips={trims.length}
          customizing={customizing}
          onCustomize={() =>
            customizing ? useChoices.getState().chooseGroup(useChoices.getState().choices?.group ?? "") : useChoices.getState().customize()
          }
          onTrim={video.duration && !trimOpen ? openTrim : undefined}
        />
      )}

      <DownloadPanel
        download={download}
        onStart={() => choice && setDownloadIds(trims.map((trim) => start(video, choice, trim)))}
        disabled={!plan.isSuccess || blocked !== null}
        label={
          blocked
            ? "Too long for this limit"
            : trims.length > 1
              ? `Download ${trims.length} clips`
              : shownTrim
                ? "Download clip"
                : "Download"
        }
      />
    </>
  );
}
