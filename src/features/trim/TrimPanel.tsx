import { ChevronLeft, ChevronRight, Pause, Play, Plus, Trash2, X } from "lucide-react";
import { AnimatePresence, motion, useMotionValue, useTransform } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Chip } from "@/components/ui/chip";
import { Hint } from "@/components/ui/tooltip";
import { bestVideoFormat } from "@/lib/formats";
import { spring } from "@/lib/motion";
import { formatLength, formatTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";
import { useOptions } from "@/stores/options";
import { useTrim } from "@/stores/trim";
import type { VideoInfo } from "@/types";
import type { PlayerHandle } from "./player";
import { Preview } from "./Preview";
import { TimeField } from "./TimeField";
import { Timeline } from "./Timeline";

/**
 * The trimmer: a preview player, a timeline with draggable handles, and the
 * list of parts to keep.
 *
 * Keyboard: Space plays/pauses, ←/→ move one second (Shift: five),
 * , and . step one frame, I and O set the start and end at the playhead.
 */
export function TrimPanel({ video, duration, disabled }: { video: VideoInfo; duration: number; disabled: boolean }) {
  const segments = useTrim((state) => state.segments);
  const activeId = useTrim((state) => state.activeId);
  const setRange = useTrim((state) => state.setRange);
  const setActive = useTrim((state) => state.setActive);
  const addSegment = useTrim((state) => state.addSegment);
  const removeSegment = useTrim((state) => state.removeSegment);
  const close = useTrim((state) => state.close);
  const exactCut = useOptions((state) => state.exactCut);
  const setExactCut = useOptions((state) => state.setExactCut);

  const player = useRef<PlayerHandle>(null);
  const [playing, setPlaying] = useState(false);
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
      if (target.closest("input, textarea, [role=listbox], [role=combobox]")) return;
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
    <div className="space-y-3 rounded-[1.25rem] border bg-card p-3 shadow-card">
      <div className="overflow-hidden rounded-xl bg-black">
        <div className="mx-auto max-h-80" style={{ aspectRatio: aspect, maxWidth: `${20 * aspect}rem` }}>
          <Preview
            video={video}
            ref={player}
            onReady={() => seek(useTrim.getState().segments[0]?.start ?? 0)}
            onTime={(seconds) => time.set(seconds)}
            onPlaying={setPlaying}
          />
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
          <Hint label="Close the trimmer and download the whole video">
            <Button variant="ghost" size="icon-sm" disabled={disabled} onClick={close} aria-label="Close trimmer">
              <X />
            </Button>
          </Hint>
        </div>
      </div>

      <Timeline duration={duration} time={time} onSeek={seek} />

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

      <div className="flex items-center justify-between gap-3 px-0.5">
        <Button variant="ghost" size="sm" disabled={disabled} onClick={() => addSegment(time.get(), duration)}>
          <Plus /> Add another part
        </Button>
        <div role="radiogroup" aria-label="Cut" className="flex items-center gap-0.5">
          <Hint label="Cuts on the exact frames. The clip is re-encoded once, at high quality.">
            <span>
              <Chip group="cut" tone="soft" selected={exactCut} onSelect={() => setExactCut(true)} disabled={disabled}>
                Exact cut
              </Chip>
            </span>
          </Hint>
          <Hint label="Keeps the original untouched and is quicker, but may start a few seconds early.">
            <span>
              <Chip group="cut" tone="soft" selected={!exactCut} onSelect={() => setExactCut(false)} disabled={disabled}>
                Fast cut
              </Chip>
            </span>
          </Hint>
        </div>
      </div>
    </div>
  );
}
