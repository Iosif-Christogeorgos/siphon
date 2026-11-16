import { type MotionValue, motion, useTransform } from "motion/react";
import { type PointerEvent as ReactPointerEvent, useRef } from "react";
import { formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { type Segment, useTrim } from "@/stores/trim";

/** The stretch of the video the timeline currently shows, in seconds. */
export interface TimelineView {
  start: number;
  end: number;
}

interface TimelineProps {
  duration: number;
  /** The whole video by default; narrower when zoomed in on a part. */
  view: TimelineView;
  /** Playback position in seconds. */
  time: MotionValue<number>;
  onSeek: (seconds: number) => void;
}

/**
 * The scrub bar: click or drag anywhere to move the playhead, drag a part's
 * edges to set where it starts and ends.
 */
export function Timeline({ duration, view, time, onSeek }: TimelineProps) {
  const track = useRef<HTMLDivElement>(null);
  const segments = useTrim((state) => state.segments);
  const activeId = useTrim((state) => state.activeId);
  const setRange = useTrim((state) => state.setRange);
  const setActive = useTrim((state) => state.setActive);

  const span = view.end - view.start;
  const percent = (seconds: number) => ((seconds - view.start) / span) * 100;
  const playhead = useTransform(time, (seconds) => `${percent(seconds)}%`);
  // The playhead sits outside the clipped layer (its knob pokes above the
  // track), so hide it by hand when it is outside the zoomed view.
  const playheadOpacity = useTransform(time, (seconds) =>
    seconds < view.start - 0.001 || seconds > view.end + 0.001 ? 0 : 1,
  );

  const timeAt = (clientX: number) => {
    const rect = track.current?.getBoundingClientRect();
    if (!rect) return view.start;
    return view.start + Math.min(Math.max((clientX - rect.left) / rect.width, 0), 1) * span;
  };

  /** Follows the pointer from press to release, wherever it goes. */
  const drag = (event: ReactPointerEvent, move: (seconds: number) => void) => {
    event.preventDefault();
    event.stopPropagation();
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    move(timeAt(event.clientX));
    const onMove = (moveEvent: Event) => move(timeAt((moveEvent as PointerEvent).clientX));
    const stop = () => {
      target.removeEventListener("pointermove", onMove);
      target.removeEventListener("pointerup", stop);
      target.removeEventListener("pointercancel", stop);
    };
    target.addEventListener("pointermove", onMove);
    target.addEventListener("pointerup", stop);
    target.addEventListener("pointercancel", stop);
  };

  const dragEdge = (segment: Segment, edge: "start" | "end") => (event: ReactPointerEvent) =>
    drag(event, (seconds) => {
      setRange(segment.id, { [edge]: seconds }, duration);
      // Show the frame under the handle while dragging it.
      const moved = useTrim.getState().segments.find((s) => s.id === segment.id);
      if (moved) onSeek(moved[edge]);
    });

  return (
    <div className="select-none">
      <div
        ref={track}
        onPointerDown={(event) => drag(event, onSeek)}
        className="relative h-11 cursor-pointer touch-none rounded-lg bg-secondary"
      >
        <div className="absolute inset-0 overflow-hidden rounded-lg">
          {segments.map((segment) => {
            const active = segment.id === activeId;
            return (
              <div
                key={segment.id}
                onPointerDown={() => setActive(segment.id)}
                style={{
                  left: `${percent(segment.start)}%`,
                  width: `${percent(segment.end) - percent(segment.start)}%`,
                }}
                className={cn(
                  // min-w keeps both grips reachable when the part is tiny on screen.
                  "absolute inset-y-0 min-w-5 rounded-md border-y-2 transition-colors duration-150",
                  active ? "z-10 border-primary bg-primary/20" : "border-primary/40 bg-primary/10",
                )}
              >
                <Handle side="left" active={active} onPointerDown={dragEdge(segment, "start")} />
                <Handle side="right" active={active} onPointerDown={dragEdge(segment, "end")} />
              </div>
            );
          })}
        </div>

        <motion.div
          style={{ left: playhead, opacity: playheadOpacity }}
          className="pointer-events-none absolute -inset-y-1 z-20 w-0.5 -translate-x-1/2 rounded-full bg-foreground shadow-[0_0_0_1px_var(--background)]"
        >
          <div className="absolute -top-1 left-1/2 size-2.5 -translate-x-1/2 rounded-full bg-foreground shadow-[0_0_0_1px_var(--background)]" />
        </motion.div>
      </div>

      <div className="mt-1.5 flex justify-between text-[11px] text-subtle-foreground tabular-nums">
        <span>{formatTimecode(view.start)}</span>
        <span>{formatTimecode(view.end)}</span>
      </div>
    </div>
  );
}

function Handle({
  side,
  active,
  onPointerDown,
}: {
  side: "left" | "right";
  active: boolean;
  onPointerDown: (event: ReactPointerEvent) => void;
}) {
  return (
    <div
      onPointerDown={onPointerDown}
      className={cn(
        "group absolute inset-y-0 z-10 flex w-2.5 cursor-ew-resize touch-none",
        side === "left" ? "left-0" : "right-0",
      )}
    >
      <div
        className={cn(
          "flex size-full items-center justify-center transition-colors duration-150",
          side === "left" ? "rounded-l-md" : "rounded-r-md",
          active ? "bg-primary group-hover:bg-primary-hover" : "bg-primary/50",
        )}
      >
        <div className="h-4 w-0.5 rounded-full bg-primary-foreground/70" />
      </div>
    </div>
  );
}
