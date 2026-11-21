import { Pause, Play } from "lucide-react";
import { motion, type MotionValue, useMotionValue, useTransform } from "motion/react";
import { type PointerEvent, useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { Preview } from "@/features/trim/Preview";
import type { PlayerHandle } from "@/features/trim/player";
import { bestVideoFormat } from "@/lib/formats";
import { formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { DEFAULT_PLACEMENT, placementFor, useChoices } from "@/stores/choices";
import type { Placement, VideoInfo } from "@/types";
import { ChatBox } from "./ChatBox";

/** Quick places for the box, keeping its darkness. */
const SPOTS: { label: string; hint: string; place: Omit<Placement, "shade"> }[] = [
  { label: "Right", hint: "Down the right edge", place: DEFAULT_PLACEMENT },
  { label: "Top right", hint: "The top right corner", place: { x: 0.765, y: 0.02, width: 0.22, height: 0.48 } },
  { label: "Bottom left", hint: "The bottom left corner", place: { x: 0.015, y: 0.5, width: 0.22, height: 0.48 } },
];

/** The darkest the box's background can be made: past this it hides the stream. */
const MAX_SHADE = 0.8;

/**
 * Where the chat goes: the preview, with the chat box over it showing the
 * real chat at the playhead. The place is kept for the channel, so the
 * next video from it starts there.
 *
 * Keyboard: Space plays or pauses; ← and → move five seconds.
 */
export function ChatPanel({
  video,
  channel,
  disabled,
  onDone,
}: {
  video: VideoInfo;
  channel: string;
  disabled: boolean;
  onDone: () => void;
}) {
  const placement = useChoices((state) => (state.choices ? placementFor(state.choices, channel) : DEFAULT_PLACEMENT));
  const place = useCallback((next: Placement) => useChoices.getState().placeChat(channel, next), [channel]);

  const panel = useRef<HTMLDivElement>(null);
  const player = useRef<PlayerHandle>(null);
  const [playing, setPlaying] = useState(false);
  // A stream as it airs has no timeline: the box shows its chat as it comes.
  const live = video.live !== null;
  const duration = video.duration ?? 0;
  // A moment into the video, where its chat has got going; the box asks for
  // that moment's chat straight away, while the preview loads.
  const time = useMotionValue(duration * 0.2);
  const timeLabel = useTransform(time, formatTimecode);
  const format = bestVideoFormat(video.formats);
  const aspect = format?.width && format.height ? format.width / format.height : 16 / 9;

  useEffect(() => {
    // (Braces matter: scrollIntoView returns a promise, which is not a valid effect cleanup.)
    void panel.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, []);

  const seek = useCallback(
    (seconds: number) => {
      const clamped = Math.min(Math.max(seconds, 0), duration);
      time.set(clamped);
      player.current?.seek(clamped);
    },
    [duration, time],
  );
  const togglePlay = useCallback(() => {
    if (playing) player.current?.pause();
    else player.current?.play();
  }, [playing]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      // Typing in a field, driving a menu or moving the box takes priority.
      if (target.closest("input, textarea, [role=listbox], [role=combobox], [role=radiogroup], [role=group]")) return;
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      const actions: Record<string, () => void> = {
        " ": togglePlay,
        ArrowLeft: () => seek(time.get() - 5),
        ArrowRight: () => seek(time.get() + 5),
      };
      const action = actions[event.key];
      if (!action) return;
      event.preventDefault();
      if (target instanceof HTMLButtonElement) target.blur();
      action();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [seek, time, togglePlay]);

  return (
    <div ref={panel} className="scroll-my-3 space-y-3 rounded-[1.25rem] border bg-card p-3 shadow-card">
      <div className="overflow-hidden rounded-xl bg-black">
        <div className="relative mx-auto max-h-80" style={{ aspectRatio: aspect, maxWidth: `${20 * aspect}rem` }}>
          <Preview
            video={video}
            ref={player}
            onReady={() => {
              if (!live) seek(time.get());
            }}
            onTime={(seconds) => time.set(seconds)}
            onPlaying={setPlaying}
          />
          <ChatBox
            url={video.url}
            time={time}
            live={live}
            placement={placement}
            onPlace={place}
            disabled={disabled}
          />
        </div>
      </div>

      <div className="flex items-center gap-2 px-0.5">
        <Hint label={playing ? "Pause (Space)" : "Play (Space)"}>
          <Button size="icon" onClick={togglePlay} aria-label={playing ? "Pause" : "Play"}>
            {playing ? <Pause className="fill-current" /> : <Play className="fill-current" />}
          </Button>
        </Hint>
        {live ? (
          <span className="text-xs text-muted-foreground">Live: the box shows the chat as it comes.</span>
        ) : (
          <>
            <span className="shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
              <motion.span className="text-foreground">{timeLabel}</motion.span> / {formatTimecode(duration)}
            </span>
            <Scrubber duration={duration} time={time} onSeek={seek} />
          </>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-0.5">
        <div className="flex items-center gap-1" role="toolbar" aria-label="Quick places">
          {SPOTS.map((spot) => (
            <Hint key={spot.label} label={spot.hint}>
              <Button
                variant="ghost"
                size="sm"
                disabled={disabled}
                onClick={() => place({ ...spot.place, shade: placement.shade })}
              >
                {spot.label}
              </Button>
            </Hint>
          ))}
        </div>
        <label className="flex items-center gap-2 text-[13px] text-muted-foreground">
          Background
          <input
            type="range"
            min={0}
            max={MAX_SHADE}
            step={0.05}
            value={placement.shade}
            disabled={disabled}
            onChange={(event) => place({ ...placement, shade: Number(event.target.value) })}
            className="w-28 accent-primary"
            aria-valuetext={`${Math.round(placement.shade * 100)}% dark`}
          />
          <span className="w-8 text-right tabular-nums">{Math.round(placement.shade * 100)}%</span>
        </label>
        <Button variant="secondary" size="sm" className="ml-auto" onClick={onDone}>
          Done
        </Button>
      </div>
      <p className="px-1 text-xs text-muted-foreground">
        Drag the box where the chat should go, and a corner to resize it: the text grows with its width.{" "}
        {live ? `${channel}’s recordings` : `${channel}’s videos`} start here next time.
      </p>
    </div>
  );
}

/** A plain scrub bar: click or drag anywhere to move the playhead. */
function Scrubber({
  duration,
  time,
  onSeek,
}: {
  duration: number;
  time: MotionValue<number>;
  onSeek: (seconds: number) => void;
}) {
  const track = useRef<HTMLDivElement>(null);
  const played = useTransform(time, (seconds) => `${duration > 0 ? (seconds / duration) * 100 : 0}%`);
  const seekTo = (clientX: number) => {
    const bounds = track.current?.getBoundingClientRect();
    if (!bounds || bounds.width === 0) return;
    onSeek(((clientX - bounds.left) / bounds.width) * duration);
  };
  const press = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    seekTo(event.clientX);
  };
  return (
    <div
      ref={track}
      role="slider"
      aria-label="Playhead"
      aria-valuemin={0}
      aria-valuemax={Math.round(duration)}
      aria-valuenow={Math.round(time.get())}
      tabIndex={-1}
      onPointerDown={press}
      onPointerMove={(event) => event.buttons === 1 && seekTo(event.clientX)}
      className="group/scrub relative flex h-6 min-w-0 flex-1 cursor-pointer items-center"
    >
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-secondary">
        <motion.div className="h-full bg-primary" style={{ width: played }} />
      </div>
      <motion.div
        aria-hidden
        className={cn(
          "absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-primary shadow-sm",
          "transition-transform duration-150 group-hover/scrub:scale-125",
        )}
        style={{ left: played }}
      />
    </div>
  );
}
