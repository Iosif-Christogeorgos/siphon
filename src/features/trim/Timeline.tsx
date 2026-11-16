import { type MotionValue, motion, useTransform } from "motion/react";
import { type PointerEvent as ReactPointerEvent, useRef } from "react";
import { formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { type Segment, useTrim } from "@/stores/trim";

interface TimelineProps {
  duration: number;
  /** Playback position in seconds. */
  time: MotionValue<number>;
  onSeek: (seconds: number) => void;
}

/**
 * The scrub bar: click or drag anywhere to move the playhead, drag a part's
 * edges to set where it starts and ends.
 */
export function Timeline({ duration, time, onSeek }: TimelineProps) {
  const track = useRef<HTMLDivElement>(null);
  const segments = useTrim((state) => state.segments);
  const activeId = useTrim((state) => state.activeId);
  const setRange = useTrim((state) => state.setRange);
  const setActive = useTrim((state) => state.setActive);

  const playhead = useTransform(time, (seconds) => `${(seconds / duration) * 100}%`);

  const timeAt = (clientX: number) => {
    const rect = track.current?.getBoundingClientRect();
    if (!rect) return 0;
    return Math.min(Math.max((clientX - rect.left) / rect.width, 0), 1) * duration;
  };

  /** Follows the pointer from press to release, wherever it goes. */
  const drag = (event: ReactPointerEvent, move: (seconds: number) => void) => {
    event.preventDefault();
    event.stopPropagation();
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    move(timeAt(event.clientX));
    const onMove = (moveEvent: PointerEvent) => move(timeAt(moveEvent.clientX));
    const stop = () => {
      target.removeEventListener("pointermove", onMove as EventListener);
      target.removeEventListener("pointerup", stop);
      target.removeEventListener("pointercancel", stop);
    };
    target.addEventListener("pointermove", onMove as EventListener);
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
        {segments.map((segment) => {
          const active = segment.id === activeId;
          return (
            <div
              key={segment.id}
              onPointerDown={() => setActive(segment.id)}
              style={{
                left: `${(segment.start / duration) * 100}%`,
                width: `${((segment.end - segment.start) / duration) * 100}%`,
              }}
              className={cn(
                "absolute inset-y-0 rounded-md border-y-2 transition-colors duration-150",
                active ? "z-10 border-primary bg-primary/20" : "border-primary/40 bg-primary/10",
              )}
            >
              <Handle side="left" active={active} onPointerDown={dragEdge(segment, "start")} />
              <Handle side="right" active={active} onPointerDown={dragEdge(segment, "end")} />
            </div>
          );
        })}

        <motion.div
          style={{ left: playhead }}
          className="pointer-events-none absolute -inset-y-1 z-20 w-0.5 -translate-x-1/2 rounded-full bg-foreground shadow-[0_0_0_1px_var(--background)]"
        >
          <div className="absolute -top-1 left-1/2 size-2.5 -translate-x-1/2 rounded-full bg-foreground shadow-[0_0_0_1px_var(--background)]" />
        </motion.div>
      </div>

      <div className="mt-1.5 flex justify-between text-[11px] text-subtle-foreground tabular-nums">
        <span>{formatTimecode(0)}</span>
        <span>{formatTimecode(duration)}</span>
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
        // A wide invisible hit area around a slim visible grip.
        "group absolute inset-y-0 z-10 flex w-4 cursor-ew-resize touch-none items-center justify-center",
        side === "left" ? "-left-2" : "-right-2",
      )}
    >
      <div
        className={cn(
          "h-full w-2 transition-[background-color,transform] duration-150 group-hover:scale-x-125 group-active:scale-x-125",
          side === "left" ? "rounded-l-md" : "rounded-r-md",
          active ? "bg-primary" : "bg-primary/50",
        )}
      >
        <div className="mx-auto mt-3.5 h-4 w-0.5 rounded-full bg-primary-foreground/70" />
      </div>
    </div>
  );
}
