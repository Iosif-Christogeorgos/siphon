import type { Format } from "@/types";

/**
 * The familiar "1080p60" label. Uses the shorter side so vertical video
 * (1080×1920) is still called 1080p, and only mentions frame rates above 30.
 */
export function resolutionLabel(format: Pick<Format, "width" | "height" | "fps">): string | null {
  const lines = format.width && format.height ? Math.min(format.width, format.height) : format.height;
  if (!lines) return null;
  const fps = format.fps && format.fps > 30 ? Math.round(format.fps) : "";
  return `${lines}p${fps}`;
}

/** The highest-resolution (then highest frame rate) stream that carries video. */
export function bestVideoFormat(formats: Format[]): Format | null {
  let best: Format | null = null;
  for (const format of formats) {
    if (format.kind === "audio" || !format.height) continue;
    const better =
      best === null ||
      format.height > (best.height ?? 0) ||
      (format.height === best.height && (format.fps ?? 0) > (best.fps ?? 0));
    if (better) best = format;
  }
  return best;
}
