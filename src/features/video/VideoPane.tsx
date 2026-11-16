import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { DownloadPanel } from "@/features/download/DownloadPanel";
import { ManualPanel } from "@/features/options/ManualPanel";
import { PlanCard } from "@/features/options/PlanCard";
import { PresetPicker } from "@/features/options/PresetPicker";
import { QualityPicker } from "@/features/options/QualityPicker";
import { ipc, toAppError } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { useDownloads } from "@/stores/downloads";
import { useDownloadOptions, useOptions } from "@/stores/options";
import type { VideoInfo } from "@/types";
import { VideoCard } from "./VideoCard";

/** Everything shown for a loaded video: the card, the choices, the plan and the button. */
export function VideoPane({ video }: { video: VideoInfo }) {
  const mode = useOptions((state) => state.mode);
  const options = useDownloadOptions();

  const [downloadId, setDownloadId] = useState<string | null>(null);
  const download = useDownloads((state) => (downloadId ? state.byId[downloadId] : undefined));
  const start = useDownloads((state) => state.start);
  // Choices are locked while a download runs: it was started with the ones shown.
  const running = download?.status === "running";

  // Resolution and stream picks belong to the previous video.
  useEffect(() => useOptions.getState().resetForVideo(), []);

  // Changing a choice after a download finished (or failed) brings back the Download button.
  const optionsKey = JSON.stringify(options);
  useEffect(() => {
    setDownloadId((id) => (id && useDownloads.getState().byId[id]?.status === "running" ? id : null));
  }, [optionsKey]);

  const plan = useQuery({
    queryKey: ["plan", video.url, options],
    queryFn: () => ipc.planDownload(video.url, options),
    // Keep showing the last plan while the next one is computed, so the layout doesn't jump.
    placeholderData: keepPreviousData,
  });

  return (
    <>
      <VideoCard video={video} />

      <div className="space-y-2 pt-1">
        <PresetPicker disabled={running} />
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.div
            key={mode}
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            transition={spring}
          >
            {mode === "manual" ? (
              <ManualPanel video={video} disabled={running} />
            ) : (
              <QualityPicker plan={plan.data} disabled={running} />
            )}
          </motion.div>
        </AnimatePresence>
      </div>

      {plan.isError ? <ErrorCard error={toAppError(plan.error)} /> : <PlanCard plan={plan.data} />}

      <DownloadPanel
        download={download}
        onStart={() => setDownloadId(start(video, options))}
        disabled={!plan.isSuccess}
      />
    </>
  );
}
