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
const BUFFERING = 3;
/** How long to wait for the embed before giving up on it. */
const READY_TIMEOUT_MS = 12_000;
/** Dragging a handle asks for a seek on every pointer move; the player gets at most one per interval. */
const SEEK_INTERVAL_MS = 120;
/** How long the player may take to land on a seek target before its own time is trusted again. */
const SEEK_SETTLE_MS = 1500;
/** If the player is still buffering after this long, it has stalled. */
const STALL_MS = 2500;
const MAX_STALL_NUDGES = 3;

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

  // Seek throttling: remember the latest target and send it when the interval allows.
  const seeking = useRef<{
    target: number | null;
    timer: number | undefined;
    last: number;
    /** Where the last seek was sent; the player reports its old position until it gets there. */
    awaiting: number | null;
  }>({ target: null, timer: undefined, last: 0, awaiting: null });

  useImperativeHandle(ref, () => ({
    play: () => player.current?.playVideo(),
    pause: () => player.current?.pauseVideo(),
    seek: (seconds) => {
      const state = seeking.current;
      state.target = seconds;
      if (state.timer !== undefined) return;
      const flush = () => {
        state.timer = undefined;
        if (state.target === null) return;
        player.current?.seekTo(state.target, true);
        state.awaiting = state.target;
        state.target = null;
        state.last = performance.now();
      };
      const wait = SEEK_INTERVAL_MS - (performance.now() - state.last);
      if (wait <= 0) flush();
      else state.timer = window.setTimeout(flush, wait);
    },
  }));

  useEffect(() => {
    let cancelled = false;
    let instance: YouTubePlayerInstance | null = null;
    let ticker: number | undefined;
    let stallTimer: number | undefined;
    let nudges = 0;
    // A cued video shows no frame when seeking, and a seek would start it
    // playing. So play it muted for an instant first, then pause: after
    // that, seeking simply shows the frame.
    let priming = true;

    const giveUp = window.setTimeout(() => priming && events.current.onError(), READY_TIMEOUT_MS);
    const fail = () => {
      window.clearTimeout(giveUp);
      if (!cancelled) events.current.onError();
    };

    // The embed occasionally sits in "buffering" for good after a seek.
    // Seeking again, a few frames on, reliably gets it going.
    const watchForStall = (state: number) => {
      window.clearTimeout(stallTimer);
      if (state !== BUFFERING) {
        if (state === PLAYING) nudges = 0;
        return;
      }
      if (nudges >= MAX_STALL_NUDGES) return;
      stallTimer = window.setTimeout(() => {
        if (!instance) return;
        nudges += 1;
        instance.seekTo(instance.getCurrentTime() + 0.05, true);
        instance.playVideo();
      }, STALL_MS);
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
              watchForStall(data);
              // Buffering in the middle of playback still counts as playing.
              if (data !== BUFFERING) events.current.onPlaying(data === PLAYING);
            },
            onError: fail,
          },
        });
        // The API has no time-update event, so ask for the position regularly.
        ticker = window.setInterval(() => {
          const seek = seeking.current;
          if (priming || !instance || seek.target !== null) return;
          const now = instance.getCurrentTime();
          // After a seek the player keeps reporting its old position for a
          // moment. Ignore that until it arrives (or clearly never will).
          if (seek.awaiting !== null) {
            const arrived = Math.abs(now - seek.awaiting) < 0.5;
            if (!arrived && performance.now() - seek.last < SEEK_SETTLE_MS) return;
            seek.awaiting = null;
          }
          events.current.onTime(now);
        }, 50);
      })
      .catch(fail);

    const pendingSeek = seeking.current;
    return () => {
      cancelled = true;
      window.clearTimeout(giveUp);
      window.clearTimeout(stallTimer);
      window.clearTimeout(pendingSeek.timer);
      pendingSeek.timer = undefined;
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
