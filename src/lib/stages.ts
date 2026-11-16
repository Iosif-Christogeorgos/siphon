import type { Stage } from "@/types";

/** What each stage of a download is called in the UI. */
export const STAGE_LABELS: Record<Stage, string> = {
  queued: "Waiting in the queue",
  updating: "Updating the download engine",
  retrying: "Trying again in a moment",
  preparing: "Preparing",
  downloading_video: "Downloading video",
  downloading_audio: "Downloading audio",
  downloading: "Downloading",
  merging: "Merging",
  converting: "Converting",
  finishing: "Finishing up",
};

/** Stages with no measurable progress yet: shown without a percentage. */
export const isIndeterminate = (stage: Stage) =>
  stage === "queued" || stage === "updating" || stage === "retrying" || stage === "preparing";
