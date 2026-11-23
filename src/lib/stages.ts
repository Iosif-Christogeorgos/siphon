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
  chat: "Fetching the chat",
  drawing_chat: "Drawing in the chat",
  finishing: "Finishing up",
  waiting_live: "Waiting for the stream to start",
  recording: "Recording",
  reconnecting: "Stream paused, waiting for it",
  finalizing: "Finishing the recording",
};

/** Stages that wait rather than work: their name says so, and nothing moves. */
const IDLE: ReadonlySet<Stage> = new Set(["queued", "retrying"]);

/**
 * A download at work with nothing measured yet. It shows its stage with its
 * bar moving, never a 0% that looks stuck: a recording's chat as it is
 * finished sat at 0% for minutes and looked like a crash (2026-11-23).
 */
export const isBusy = (stage: Stage, fraction: number) => fraction <= 0 && !IDLE.has(stage) && !isLive(stage);

/** "42%", once there is something to measure; until then, nothing. */
export const formatPercent = (fraction: number) =>
  fraction > 0 ? `${Math.floor(Math.min(fraction, 1) * 100)}%` : null;

/** Stages of a recording that is on air or waiting for it: there is nothing to pause. */
export const isLive = (stage: Stage) => stage === "waiting_live" || stage === "recording" || stage === "reconnecting";
