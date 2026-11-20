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
  subtitles: "Adding subtitles",
  finishing: "Finishing up",
  waiting_live: "Waiting for the stream to start",
  recording: "Recording",
  reconnecting: "Stream paused, waiting for it",
  finalizing: "Finishing the recording",
};

/** Stages with no measurable progress yet: shown without a percentage. */
export const isIndeterminate = (stage: Stage) =>
  stage === "queued" ||
  stage === "updating" ||
  stage === "retrying" ||
  stage === "preparing" ||
  stage === "waiting_live" ||
  stage === "recording" ||
  stage === "reconnecting";

/** Stages of a recording that is on air or waiting for it: there is nothing to pause. */
export const isLive = (stage: Stage) => stage === "waiting_live" || stage === "recording" || stage === "reconnecting";
