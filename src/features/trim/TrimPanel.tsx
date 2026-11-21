import { ChevronLeft, ChevronRight, Pause, Play, Plus, Trash2, X, ZoomIn, ZoomOut } from "lucide-react";
import { AnimatePresence, motion, useMotionValue, useTransform } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { ChatBox } from "@/features/chat/ChatBox";
import { bestVideoFormat } from "@/lib/formats";
import { spring } from "@/lib/motion";
import { formatLength, formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { placementFor, useChoices } from "@/stores/choices";
import { useTrim } from "@/stores/trim";
import type { VideoInfo } from "@/types";
import type { PlayerHandle } from "./player";
import { Preview } from "./Preview";
import { TimeField } from "./TimeField";
import { Timeline, type TimelineView } from "./Timeline";

/**
 * The trimmer: a preview player, a timeline with draggable handles, and the
 * list of parts to keep.
 *
 * Keyboard: Space plays/pauses, ←/→ move one second (Shift: five),
 * , and . step one frame, I and O set the start and end at the playhead.
 * Playback started inside a part stops at its end, so Space previews the clip.
 */
export function TrimPanel({
  video,
  duration,
  disabled,
  chatChannel = null,
}: {
  video: VideoInfo;
  duration: number;
  disabled: boolean;
  /** The channel whose chat is drawn into the clips, shown over the preview as it will be. */
  chatChannel?: string | null;
}) {
  const segments = useTrim((state) => state.segments);
  const activeId = useTrim((state) => state.activeId);
  const setRange = useTrim((state) => state.setRange);
  const setActive = useTrim((state) => state.setActive);
  const addSegment = useTrim((state) => state.addSegment);
  const removeSegment = useTrim((state) => state.removeSegment);
  const close = useTrim((state) => state.close);

  const chatPlacement = useChoices((state) =>
    chatChannel && state.choices ? placementFor(state.choices, chatChannel) : null,
  );
  const panel = useRef<HTMLDivElement>(null);
  const player = useRef<PlayerHandle>(null);
  const [playing, setPlaying] = useState(false);
  // null shows the whole video; otherwise the timeline is zoomed to this stretch.
  const [zoom, setZoom] = useState<TimelineView | null>(null);
  const view = zoom ?? { start: 0, end: duration };

  // The panel opens below the fold on smaller windows; bring it into view.
  useEffect(() => {
    // (Braces matter: scrollIntoView returns a promise, which is not a valid effect cleanup.)
    void panel.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, []);
  // A motion value, not state: the position changes every frame while playing.
  const time = useMotionValue(0);
  const timeLabel = useTransform(time, formatTimecode);

  const format = bestVideoFormat(video.formats);
  const frame = 1 / (format?.fps && format.fps > 0 ? format.fps : 30);
  const aspect = format?.width && format.height ? format.width / format.height : 16 / 9;

  const seek = useCallback(
    (seconds: number) => {
      const clamped = Math.min(Math.max(seconds, 0), duration);
      // Move the playhead at once; the player catches up.
      time.set(clamped);
      player.current?.seek(clamped);
    },
    [duration, time],
  );
  /** Follows playback, and pauses at the end of the part it started in. */
  const onTime = useCallback(
    (seconds: number) => {
      const previous = time.get();
      time.set(seconds);
      const { segments, activeId } = useTrim.getState();
      const end = segments.find((segment) => segment.id === activeId)?.end;
      if (end !== undefined && end < duration && previous < end && seconds >= end && seconds - previous < 1) {
        player.current?.pause();
      }
    },
    [duration, time],
  );
  /** Zooms the timeline in on the active part, or back out to the whole video. */
  const toggleZoom = () => {
    const active = segments.find((segment) => segment.id === activeId);
    if (zoom || !active) return setZoom(null);
    const margin = Math.max((active.end - active.start) * 0.75, 2);
    setZoom({ start: Math.max(0, active.start - margin), end: Math.min(duration, active.end + margin) });
  };
  const togglePlay = useCallback(() => {
    if (playing) player.current?.pause();
    else player.current?.play();
  }, [playing]);
  const setEdge = useCallback(
    (edge: "start" | "end") => {
      const id = useTrim.getState().activeId;
      if (id) setRange(id, { [edge]: time.get() }, duration);
    },
    [duration, setRange, time],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      // Typing in a field or driving a menu takes priority.
      if (target.closest("input, textarea, [role=listbox], [role=combobox], [role=radiogroup], [role=group]")) return;
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      const now = time.get();
      const actions: Record<string, () => void> = {
        " ": togglePlay,
        ArrowLeft: () => seek(now - (event.shiftKey ? 5 : 1)),
        ArrowRight: () => seek(now + (event.shiftKey ? 5 : 1)),
        ",": () => seek(now - frame),
        ".": () => seek(now + frame),
        i: () => setEdge("start"),
        o: () => setEdge("end"),
      };
      const action = actions[event.key.length === 1 ? event.key.toLowerCase() : event.key];
      if (!action) return;
      event.preventDefault();
      // Otherwise Space would also "click" whichever button has focus.
      if (target instanceof HTMLButtonElement) target.blur();
      action();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [frame, seek, setEdge, time, togglePlay]);

  return (
    <div ref={panel} className="scroll-my-3 space-y-3 rounded-[1.25rem] border bg-card p-3 shadow-card">
      <div className="overflow-hidden rounded-xl bg-black">
        <div className="relative mx-auto max-h-80" style={{ aspectRatio: aspect, maxWidth: `${20 * aspect}rem` }}>
          <Preview
            video={video}
            ref={player}
            onReady={() => seek(useTrim.getState().segments[0]?.start ?? 0)}
            onTime={onTime}
            onPlaying={setPlaying}
          />
          {chatChannel && chatPlacement && (
            <ChatBox
              url={video.url}
              time={time}
              placement={chatPlacement}
              onPlace={(placement) => useChoices.getState().placeChat(chatChannel, placement)}
              disabled={disabled}
            />
          )}
        </div>
      </div>

      <div className="flex items-center gap-1 px-0.5">
        <Hint label={playing ? "Pause (Space)" : "Play (Space)"}>
          <Button size="icon" onClick={togglePlay} aria-label={playing ? "Pause" : "Play"}>
            {playing ? <Pause className="fill-current" /> : <Play className="fill-current" />}
          </Button>
        </Hint>
        <Hint label="Back one frame (,)">
          <Button variant="ghost" size="icon-sm" onClick={() => seek(time.get() - frame)} aria-label="Back one frame">
            <ChevronLeft />
          </Button>
        </Hint>
        <Hint label="Forward one frame (.)">
          <Button variant="ghost" size="icon-sm" onClick={() => seek(time.get() + frame)} aria-label="Forward one frame">
            <ChevronRight />
          </Button>
        </Hint>
        <span className="ml-1.5 font-mono text-xs text-muted-foreground tabular-nums">
          <motion.span className="text-foreground">{timeLabel}</motion.span> / {formatTimecode(duration)}
        </span>

        <div className="ml-auto flex items-center gap-1">
          <Hint label="Start the part here (I)">
            <Button variant="secondary" size="sm" disabled={disabled} onClick={() => setEdge("start")}>
              Set start
            </Button>
          </Hint>
          <Hint label="End the part here (O)">
            <Button variant="secondary" size="sm" disabled={disabled} onClick={() => setEdge("end")}>
              Set end
            </Button>
          </Hint>
          <Hint label={zoom ? "Show the whole video" : "Zoom the timeline in on this part"}>
            <Button variant="ghost" size="icon-sm" onClick={toggleZoom} aria-label={zoom ? "Zoom out" : "Zoom in"}>
              {zoom ? <ZoomOut /> : <ZoomIn />}
            </Button>
          </Hint>
          <Hint label="Close the trimmer and download the whole video">
            <Button variant="ghost" size="icon-sm" disabled={disabled} onClick={close} aria-label="Close trimmer">
              <X />
            </Button>
          </Hint>
        </div>
      </div>

      <Timeline duration={duration} view={view} time={time} onSeek={seek} />

      <ul className="space-y-1">
        <AnimatePresence initial={false}>
          {segments.map((segment, index) => (
            <motion.li
              key={segment.id}
              layout
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={spring}
              className="overflow-hidden"
            >
              <div
                onClick={() => setActive(segment.id)}
                className={cn(
                  "flex items-center gap-2 rounded-lg py-1 pr-1 pl-2.5 transition-colors duration-150",
                  segment.id === activeId ? "bg-primary-soft" : "hover:bg-accent",
                )}
              >
                <span className="w-9 text-xs font-medium text-muted-foreground">Part {index + 1}</span>
                <TimeField
                  label={`Part ${index + 1} start`}
                  value={segment.start}
                  frame={frame}
                  disabled={disabled}
                  onCommit={(start) => {
                    setRange(segment.id, { start }, duration);
                    seek(start);
                  }}
                />
                <span className="text-subtle-foreground">→</span>
                <TimeField
                  label={`Part ${index + 1} end`}
                  value={segment.end}
                  frame={frame}
                  disabled={disabled}
                  onCommit={(end) => {
                    setRange(segment.id, { end }, duration);
                    seek(end);
                  }}
                />
                <span className="text-xs text-muted-foreground tabular-nums">
                  {formatLength(segment.end - segment.start)}
                </span>
                {segments.length > 1 && (
                  <Hint label="Remove this part">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="ml-auto"
                      disabled={disabled}
                      onClick={(event) => {
                        event.stopPropagation();
                        removeSegment(segment.id);
                      }}
                      aria-label={`Remove part ${index + 1}`}
                    >
                      <Trash2 />
                    </Button>
                  </Hint>
                )}
              </div>
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>

      <Button variant="ghost" size="sm" disabled={disabled} onClick={() => addSegment(time.get(), duration)}>
        <Plus /> Add another part
      </Button>
    </div>
  );
}
