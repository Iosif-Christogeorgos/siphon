import { type Ref, useEffect, useImperativeHandle, useRef } from "react";
import type { PlayerEvents, PlayerHandle } from "./player";

// The slice of YouTube's IFrame Player API that the preview uses.
// https://developers.google.com/youtube/iframe_api_reference
interface YouTubePlayerInstance {
  playVideo(): void;
  pauseVideo(): void;
  seekTo(seconds: number, allowSeekAhead: boolean): void;
  getCurrentTime(): number;
  mute(): void;
  unMute(): void;
  destroy(): void;
}
interface YouTubeApi {
  Player: new (
    element: HTMLElement,
    options: {
      videoId: string;
      width: string;
      height: string;
      playerVars: Record<string, string | number>;
      events: {
        onReady: () => void;
        onStateChange: (event: { data: number }) => void;
        onError: () => void;
      };
    },
  ) => YouTubePlayerInstance;
}
declare global {
  interface Window {
    YT?: YouTubeApi;
    onYouTubeIframeAPIReady?: () => void;
  }
}

const PLAYING = 1;
/** How long to wait for the embed before giving up on it. */
const READY_TIMEOUT_MS = 12_000;

let apiLoading: Promise<YouTubeApi> | null = null;

/** Loads YouTube's API script once per session. */
function loadApi(): Promise<YouTubeApi> {
  apiLoading ??= new Promise((resolve, reject) => {
    if (window.YT?.Player) return resolve(window.YT);
    window.onYouTubeIframeAPIReady = () => (window.YT ? resolve(window.YT) : reject());
    const script = document.createElement("script");
    script.src = "https://www.youtube.com/iframe_api";
    script.onerror = () => {
      apiLoading = null;
      reject();
    };
    document.head.append(script);
  });
  return apiLoading;
}

/** Preview through YouTube's own embedded player. */
export function YouTubePlayer({
  videoId,
  ref,
  onReady,
  onTime,
  onPlaying,
  onError,
}: { videoId: string; ref: Ref<PlayerHandle> } & PlayerEvents) {
  const mount = useRef<HTMLDivElement>(null);
  const player = useRef<YouTubePlayerInstance | null>(null);
  const events = useRef({ onReady, onTime, onPlaying, onError });
  events.current = { onReady, onTime, onPlaying, onError };

  useImperativeHandle(ref, () => ({
    play: () => player.current?.playVideo(),
    pause: () => player.current?.pauseVideo(),
    seek: (seconds) => player.current?.seekTo(seconds, true),
  }));

  useEffect(() => {
    let cancelled = false;
    let instance: YouTubePlayerInstance | null = null;
    let ticker: number | undefined;
    // A cued video shows no frame when seeking, and a seek would start it
    // playing. So play it muted for an instant first, then pause: after
    // that, seeking simply shows the frame.
    let priming = true;

    const giveUp = window.setTimeout(() => priming && events.current.onError(), READY_TIMEOUT_MS);
    const fail = () => {
      window.clearTimeout(giveUp);
      if (!cancelled) events.current.onError();
    };

    loadApi()
      .then((api) => {
        if (cancelled || !mount.current) return;
        // The API replaces this element with the iframe.
        const target = document.createElement("div");
        mount.current.append(target);
        instance = new api.Player(target, {
          videoId,
          width: "100%",
          height: "100%",
          playerVars: {
            controls: 0,
            disablekb: 1,
            fs: 0,
            rel: 0,
            iv_load_policy: 3,
            playsinline: 1,
            origin: window.location.origin,
          },
          events: {
            onReady: () => {
              instance?.mute();
              instance?.playVideo();
            },
            onStateChange: ({ data }) => {
              if (priming) {
                if (data !== PLAYING) return;
                priming = false;
                window.clearTimeout(giveUp);
                instance?.pauseVideo();
                instance?.seekTo(0, true);
                instance?.unMute();
                player.current = instance;
                events.current.onReady();
                return;
              }
              events.current.onPlaying(data === PLAYING);
            },
            onError: fail,
          },
        });
        // The API has no time-update event, so ask for the position regularly.
        ticker = window.setInterval(() => {
          if (!priming && instance) events.current.onTime(instance.getCurrentTime());
        }, 50);
      })
      .catch(fail);

    return () => {
      cancelled = true;
      window.clearTimeout(giveUp);
      window.clearInterval(ticker);
      player.current = null;
      instance?.destroy();
    };
  }, [videoId]);

  return (
    <div className="relative size-full">
      <div ref={mount} className="size-full [&>iframe]:size-full" />
      {/* Keeps clicks away from YouTube's own controls; the trim UI drives playback. */}
      <div className="absolute inset-0" />
    </div>
  );
}
