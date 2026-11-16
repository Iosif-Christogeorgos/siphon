import type { ReactNode } from "react";
import { Chip } from "@/components/ui/chip";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { formatBytes } from "@/lib/format";
import {
  AUDIO_CODEC_LABELS,
  audioFormats,
  CONTAINER_LABELS,
  describeAudioFormat,
  describeVideoFormat,
  VIDEO_CODEC_LABELS,
  videoFormats,
} from "@/lib/formats";
import { useOptions } from "@/stores/options";
import type { AudioCodec, Container, Content, Format, VideoCodec, VideoInfo } from "@/types";

const CONTENT: { id: Content; label: string }[] = [
  { id: "video_audio", label: "Video + audio" },
  { id: "video_only", label: "Video only" },
  { id: "audio_only", label: "Audio only" },
];

const VIDEO_CONTAINERS: Container[] = ["mp4", "mkv", "webm", "mov"];
const AUDIO_CONTAINERS: Container[] = ["m4a", "mp3", "opus", "ogg", "flac", "wav"];
// Conversion targets. "Keep original" is always offered first and is the lossless choice.
const VIDEO_CODECS: VideoCodec[] = ["h264", "hevc"];
const AUDIO_CODECS: AudioCodec[] = ["aac", "mp3", "opus", "flac", "pcm"];

/** Radix selects need a non-empty value; this one stands for `null`. */
const AUTO = "auto";
const toValue = (value: string | null) => value ?? AUTO;
const fromValue = <T extends string>(value: string) => (value === AUTO ? null : (value as T));

const sizeHint = (format: Format) =>
  format.filesize ? `${format.filesize_estimated ? "≈ " : ""}${formatBytes(format.filesize)}` : undefined;

/** Manual mode: pick the exact streams, container and codecs. */
export function ManualPanel({ video, disabled }: { video: VideoInfo; disabled: boolean }) {
  const manual = useOptions((state) => state.manual);
  const setManual = useOptions((state) => state.setManual);

  const videos = videoFormats(video.formats);
  const audios = audioFormats(video.formats);
  const hasVideo = videos.length > 0;
  const audioOnly = manual.content === "audio_only" || !hasVideo;
  const wantsAudio = manual.content !== "video_only";

  return (
    <div className="space-y-3 rounded-2xl border bg-card p-3.5 shadow-card">
      {hasVideo && (
        <div role="radiogroup" aria-label="Content" className="flex gap-0.5">
          {CONTENT.map(({ id, label }) => (
            <Chip
              key={id}
              group="content"
              tone="soft"
              selected={manual.content === id}
              onSelect={() => setManual({ content: id })}
              disabled={disabled}
            >
              {label}
            </Chip>
          ))}
        </div>
      )}

      <div className="grid grid-cols-6 gap-x-3 gap-y-2.5">
        {!audioOnly && (
          <Field label="Video stream" wide>
            <Select
              value={toValue(manual.video_format)}
              onValueChange={(value) => setManual({ video_format: fromValue(value) })}
              disabled={disabled}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={AUTO}>Best available</SelectItem>
                {videos.map((format) => (
                  <SelectItem key={format.id} value={format.id} hint={sizeHint(format)}>
                    {describeVideoFormat(format)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        )}

        {wantsAudio && audios.length > 0 && (
          <Field label="Audio stream" wide>
            <Select
              value={toValue(manual.audio_format)}
              onValueChange={(value) => setManual({ audio_format: fromValue(value) })}
              disabled={disabled}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={AUTO}>Best available</SelectItem>
                {audios.map((format) => (
                  <SelectItem key={format.id} value={format.id} hint={sizeHint(format)}>
                    {describeAudioFormat(format)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        )}

        <Field label="File type">
          <Select
            value={toValue(manual.container)}
            onValueChange={(value) => setManual({ container: fromValue<Container>(value) })}
            disabled={disabled}
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={AUTO}>Automatic</SelectItem>
              {(audioOnly ? AUDIO_CONTAINERS : VIDEO_CONTAINERS).map((container) => (
                <SelectItem key={container} value={container}>
                  {CONTAINER_LABELS[container]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>

        {!audioOnly && (
          <Field label="Video codec">
            <Select
              value={toValue(manual.video_codec)}
              onValueChange={(value) => setManual({ video_codec: fromValue<VideoCodec>(value) })}
              disabled={disabled}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={AUTO}>Keep original</SelectItem>
                {VIDEO_CODECS.map((codec) => (
                  <SelectItem key={codec} value={codec}>
                    Convert to {VIDEO_CODEC_LABELS[codec]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        )}

        {wantsAudio && (
          <Field label="Audio codec">
            <Select
              value={toValue(manual.audio_codec)}
              onValueChange={(value) => setManual({ audio_codec: fromValue<AudioCodec>(value) })}
              disabled={disabled}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={AUTO}>Keep original</SelectItem>
                {AUDIO_CODECS.map((codec) => (
                  <SelectItem key={codec} value={codec}>
                    Convert to {AUDIO_CODEC_LABELS[codec]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        )}
      </div>
    </div>
  );
}

function Field({ label, wide, children }: { label: string; wide?: boolean; children: ReactNode }) {
  return (
    <div className={wide ? "col-span-3" : "col-span-2"}>
      <span className="mb-1 block text-xs text-subtle-foreground">{label}</span>
      {children}
    </div>
  );
}
