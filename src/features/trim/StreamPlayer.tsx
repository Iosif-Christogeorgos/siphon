import { type Ref, useEffect, useImperativeHandle, useRef } from "react";
import type { PlayerEvents, PlayerHandle } from "./player";

/** Preview through a plain `<video>` element: a stream URL or the cached proxy file. */
export function StreamPlayer({
  src,
  ref,
  onReady,
  onTime,
  onPlaying,
  onError,
}: { src: string; ref: Ref<PlayerHandle> } & PlayerEvents) {
  const video = useRef<HTMLVideoElement>(null);
  const reportTime = useRef(onTime);
  reportTime.current = onTime;

  useImperativeHandle(ref, () => ({
    play: () => void video.current?.play().catch(() => {}),
    pause: () => video.current?.pause(),
    seek: (seconds) => {
      if (video.current) video.current.currentTime = seconds;
    },
  }));

  // `timeupdate` fires only a few times a second; follow the position every
  // frame instead so the playhead glides.
  useEffect(() => {
    let frame = requestAnimationFrame(function tick() {
      const element = video.current;
      if (element && !element.paused) reportTime.current(element.currentTime);
      frame = requestAnimationFrame(tick);
    });
    return () => cancelAnimationFrame(frame);
  }, []);

  return (
    <video
      ref={video}
      src={src}
      preload="auto"
      playsInline
      className="size-full bg-black object-contain"
      onLoadedMetadata={onReady}
      onSeeked={(event) => onTime(event.currentTarget.currentTime)}
      onPlay={() => onPlaying(true)}
      onPause={() => onPlaying(false)}
      onError={onError}
    />
  );
}
