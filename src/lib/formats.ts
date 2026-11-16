import type { AudioCodec, Container, Format, VideoCodec } from "@/types";

export const VIDEO_CODEC_LABELS: Record<VideoCodec, string> = {
  h264: "H.264",
  hevc: "HEVC",
  vp9: "VP9",
  av1: "AV1",
  vp8: "VP8",
  dnxhr: "DNxHR",
  other: "Video",
};

export const AUDIO_CODEC_LABELS: Record<AudioCodec, string> = {
  aac: "AAC",
  opus: "Opus",
  mp3: "MP3",
  vorbis: "Vorbis",
  flac: "FLAC",
  ac3: "Dolby Digital",
  pcm: "WAV (uncompressed)",
  other: "Audio",
};

export const CONTAINER_LABELS: Record<Container, string> = {
  mp4: "MP4",
  mkv: "MKV",
  webm: "WebM",
  mov: "MOV",
  m4a: "M4A",
  mp3: "MP3",
  opus: "Opus",
  ogg: "Ogg",
  flac: "FLAC",
  wav: "WAV",
};

/** Short side of the frame: 1080 for both 1920×1080 and vertical 1080×1920. */
export function linesOf(format: Pick<Format, "width" | "height">): number {
  if (format.width && format.height) return Math.min(format.width, format.height);
  return format.height ?? 0;
}

/** The familiar "1080p60" label; only mentions frame rates above 30. */
export function resolutionLabel(format: Pick<Format, "width" | "height" | "fps">): string | null {
  const lines = linesOf(format);
  if (!lines) return null;
  const fps = format.fps && format.fps > 30.5 ? Math.round(format.fps) : "";
  return `${lines}p${fps}`;
}

const isHdr = (format: Format) => format.dynamic_range !== null && format.dynamic_range !== "SDR";

/** Streams that carry video, best first. */
export function videoFormats(formats: Format[]): Format[] {
  return formats
    .filter((format) => format.kind !== "audio")
    .sort(
      (a, b) =>
        linesOf(b) - linesOf(a) || (b.fps ?? 0) - (a.fps ?? 0) || (b.tbr ?? 0) - (a.tbr ?? 0),
    );
}

/** Audio-only streams, best first. */
export function audioFormats(formats: Format[]): Format[] {
  return formats
    .filter((format) => format.kind === "audio")
    .sort(
      (a, b) =>
        (b.language_preference ?? 0) - (a.language_preference ?? 0) ||
        (b.abr ?? b.tbr ?? 0) - (a.abr ?? a.tbr ?? 0),
    );
}

/** "2160p60 · AV1 · HDR" */
export function describeVideoFormat(format: Format): string {
  return [
    resolutionLabel(format) ?? format.note ?? format.id,
    format.video_codec && VIDEO_CODEC_LABELS[format.video_codec],
    isHdr(format) && "HDR",
    format.kind === "muxed" && "with audio",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** "Opus · 160 kbps · en" */
export function describeAudioFormat(format: Format): string {
  const kbps = format.abr ?? format.tbr;
  return [
    format.audio_codec && AUDIO_CODEC_LABELS[format.audio_codec],
    kbps && `${Math.round(kbps)} kbps`,
    format.language,
    format.id.includes("drc") && "levelled volume",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** The highest-resolution (then highest frame rate) stream that carries video. */
export function bestVideoFormat(formats: Format[]): Format | null {
  return videoFormats(formats).find((format) => linesOf(format) > 0) ?? null;
}
