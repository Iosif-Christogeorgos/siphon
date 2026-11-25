import { useQuery } from "@tanstack/react-query";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AudioLines, LoaderCircle, VideoOff } from "lucide-react";
import { motion } from "motion/react";
import { type Ref, useState } from "react";
import { ipc } from "@/lib/ipc";
import type { VideoInfo } from "@/types";
import type { PlayerEvents, PlayerHandle } from "./player";
import { StreamPlayer } from "./StreamPlayer";
import { YouTubePlayer } from "./YouTubePlayer";

/** Mirrors `LAST_LEVEL` in preview.rs: the downloaded proxy is the last resort. */
const LAST_LEVEL = 2;

/**
 * The preview picture. Works down a ladder of sources until one plays:
 * YouTube's embed, a direct stream, then a small downloaded copy.
 */
export function Preview({
  video,
  ref,
  ...events
}: { video: VideoInfo; ref: Ref<PlayerHandle> } & Omit<PlayerEvents, "onError">) {
  const [level, setLevel] = useState(0);
  const preview = useQuery({
    queryKey: ["preview", video.url, level],
    queryFn: () => ipc.previewSource(video.url, level),
    staleTime: Number.POSITIVE_INFINITY,
  });

  const source = preview.data?.source;
  const exhausted = preview.isError || level > LAST_LEVEL;
  const tryNext = () => setLevel((preview.data?.level ?? level) + 1);

  if (exhausted) {
    return (
      <Notice>
        <VideoOff className="size-5" />
        <span>No preview for this video. You can still type the start and end times.</span>
      </Notice>
    );
  }
  if (!source) {
    return (
      <Notice>
        <LoaderCircle className="size-5 animate-spin" />
        {/* Only the small copy takes long enough to need explaining. A file's is made here, and
            asked for at once (a file WebView2 plays opens within the second, before this shows). */}
        {video.file ? (
          <motion.span initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ delay: 1, duration: 0.3 }}>
            Making a small copy to preview…
          </motion.span>
        ) : (
          level >= LAST_LEVEL && <span>Fetching a small copy to preview…</span>
        )}
      </Notice>
    );
  }
  if (source.kind === "youtube") {
    return <YouTubePlayer key={source.id} videoId={source.id} ref={ref} onError={tryNext} {...events} />;
  }
  const src = source.kind === "file" ? convertFileSrc(source.path) : source.url;
  const player = <StreamPlayer key={src} src={src} ref={ref} onError={tryNext} {...events} />;
  // A sound file has no picture: its cover, or what stands for sound, in its place.
  if (video.file && video.formats[0]?.kind === "audio") {
    return (
      <div className="relative size-full">
        {player}
        <div className="pointer-events-none absolute inset-0 grid place-items-center bg-black">
          {video.thumbnail ? (
            <img src={video.thumbnail} alt="" draggable={false} className="size-full object-contain" />
          ) : (
            <AudioLines className="size-8 text-white/40" />
          )}
        </div>
      </div>
    );
  }
  return player;
}

function Notice({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex size-full flex-col items-center justify-center gap-2 px-8 text-center text-[13px] text-white/60">
      {children}
    </div>
  );
}
