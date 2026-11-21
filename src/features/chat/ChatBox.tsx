import { LoaderCircle } from "lucide-react";
import type { MotionValue } from "motion/react";
import { type KeyboardEvent, type PointerEvent, useCallback, useEffect, useRef, useState } from "react";
import { ipc, toAppError } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import type { Placement } from "@/types";

/** The smallest box, as a share of the frame: narrower and a line holds a word or two. */
const MIN_SIZE = 0.08;
/** At most this often while the preview plays, in milliseconds: the box redraws a little behind. */
const REDRAW_EVERY = 80;
/** Arrow keys move the box this far (Shift: five times as far). */
const NUDGE = 0.01;
/** A redraw that takes longer than this (milliseconds) shows that it is coming. */
const SLOW = 400;
/** A live chat's box is drawn again this often (milliseconds), playing or not. */
const LIVE_EVERY = 1000;

type Corner = "nw" | "ne" | "sw" | "se";
type Grip = "move" | Corner;

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

/** The box moved by `dx`, `dy` (shares of the frame) with `grip`, kept inside the frame. */
function moved(from: Placement, grip: Grip, dx: number, dy: number): Placement {
  if (grip === "move") {
    return {
      ...from,
      x: clamp(from.x + dx, 0, 1 - from.width),
      y: clamp(from.y + dy, 0, 1 - from.height),
    };
  }
  const right = from.x + from.width;
  const bottom = from.y + from.height;
  const left = grip === "nw" || grip === "sw" ? clamp(from.x + dx, 0, right - MIN_SIZE) : from.x;
  const top = grip === "nw" || grip === "ne" ? clamp(from.y + dy, 0, bottom - MIN_SIZE) : from.y;
  const width = grip === "ne" || grip === "se" ? clamp(from.width + dx, MIN_SIZE, 1 - from.x) : right - left;
  const height = grip === "sw" || grip === "se" ? clamp(from.height + dy, MIN_SIZE, 1 - from.y) : bottom - top;
  return { ...from, x: left, y: top, width, height };
}

/** Puts what `chat_preview` answered into `canvas`: the box's pixels, at their own size. */
function draw(canvas: HTMLCanvasElement, answer: ArrayBuffer) {
  if (answer.byteLength < 16) return;
  const header = new DataView(answer, 0, 16);
  const width = header.getUint32(8, true);
  const height = header.getUint32(12, true);
  if (width === 0 || height === 0 || answer.byteLength !== 16 + width * height * 4) return;
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
  const pixels = new Uint8ClampedArray(answer, 16, width * height * 4);
  canvas.getContext("2d")?.putImageData(new ImageData(pixels, width, height), 0, 0);
}

/**
 * The chat box over the preview: the real chat at the playhead, drawn by the
 * same code that draws it into the file, so what shows here is what the file
 * gets. Drag it to move it, drag a corner to resize it (the text grows and
 * shrinks with its width), or move it with the arrow keys.
 *
 * Put it in a `relative` container the size of the video's frame.
 */
export function ChatBox({
  url,
  time,
  live = false,
  placement,
  onPlace,
  disabled = false,
}: {
  /** The video, as looked up. */
  url: string;
  /** The playhead, in seconds. */
  time: MotionValue<number>;
  /** A stream as it airs: the box shows its chat as it comes, drawn again every second. */
  live?: boolean;
  placement: Placement;
  /** The box was let go of somewhere new, or resized. */
  onPlace: (placement: Placement) => void;
  disabled?: boolean;
}) {
  const frame = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  // While dragging, the box follows the pointer and is drawn again only once it is let go.
  const [draft, setDraft] = useState<Placement | null>(null);
  const draftRef = useRef<Placement | null>(null);
  const shown = draft ?? placement;
  const [status, setStatus] = useState<"ready" | { error: string }>("ready");
  // Fetching the chat for a moment it doesn't have takes a few seconds: say so.
  const [waiting, setWaiting] = useState(true);

  // ── Drawing: one request at a time, always for the latest moment and place ──
  const wanted = useRef<{ at: number; placement: Placement; width: number; height: number } | null>(null);
  const drawn = useRef("");
  const busy = useRef(false);
  const pump = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      for (;;) {
        const job = wanted.current;
        const key = job ? JSON.stringify(job) : "";
        if (!job || key === drawn.current) break;
        const slow = window.setTimeout(() => setWaiting(true), SLOW);
        const answer = await ipc.chatPreview(url, job.at, job.placement, job.width, job.height).finally(() => {
          window.clearTimeout(slow);
        });
        if (canvas.current) draw(canvas.current, answer);
        drawn.current = key;
        setWaiting(false);
        setStatus("ready");
        await new Promise((resolve) => window.setTimeout(resolve, REDRAW_EVERY));
      }
    } catch (error) {
      setWaiting(false);
      setStatus({ error: toAppError(error).message });
    } finally {
      busy.current = false;
    }
  }, [url]);
  /** Asks for the box at the playhead, in the frame's size in device pixels. */
  const request = useCallback(() => {
    const bounds = frame.current?.getBoundingClientRect();
    if (!bounds || bounds.width === 0) return;
    const scale = window.devicePixelRatio || 1;
    wanted.current = {
      // A twentieth of a second is finer than chat moves; a live box is new every time.
      at: live ? Date.now() / 1000 : Math.round(time.get() * 20) / 20,
      placement,
      width: Math.round(bounds.width * scale),
      height: Math.round(bounds.height * scale),
    };
    void pump();
  }, [live, placement, pump, time]);

  useEffect(() => {
    request();
    const unsubscribe = time.on("change", request);
    const resized = new ResizeObserver(request);
    if (frame.current) resized.observe(frame.current);
    const ticking = live ? window.setInterval(request, LIVE_EVERY) : undefined;
    return () => {
      unsubscribe();
      resized.disconnect();
      window.clearInterval(ticking);
    };
  }, [live, request, time]);

  // ── Moving and resizing ──
  const grab = (grip: Grip) => (event: PointerEvent<HTMLElement>) => {
    if (disabled || event.button !== 0) return;
    const bounds = frame.current?.getBoundingClientRect();
    if (!bounds) return;
    event.preventDefault();
    event.stopPropagation();
    const start = { x: event.clientX, y: event.clientY, from: placement };
    const follow = (move: globalThis.PointerEvent) => {
      const next = moved(start.from, grip, (move.clientX - start.x) / bounds.width, (move.clientY - start.y) / bounds.height);
      draftRef.current = next;
      setDraft(next);
    };
    const release = () => {
      window.removeEventListener("pointermove", follow);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
      if (draftRef.current) onPlace(draftRef.current);
      draftRef.current = null;
      setDraft(null);
    };
    window.addEventListener("pointermove", follow);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
  };
  const nudge = (event: KeyboardEvent) => {
    const step = event.shiftKey ? NUDGE * 5 : NUDGE;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, -step],
      ArrowDown: [0, step],
    };
    const move = moves[event.key];
    if (!move || disabled) return;
    event.preventDefault();
    event.stopPropagation();
    onPlace(moved(placement, "move", move[0], move[1]));
  };

  const dragging = draft !== null;
  return (
    <div ref={frame} className="pointer-events-none absolute inset-0">
      <div
        role="group"
        aria-label="Chat box. Drag it, or move it with the arrow keys; drag a corner to resize it."
        tabIndex={disabled ? -1 : 0}
        onPointerDown={grab("move")}
        onKeyDown={nudge}
        className={cn(
          "group/box pointer-events-auto absolute rounded-[3px] outline-none",
          "shadow-[0_0_0_1px_rgb(0_0_0/0.35)] ring-1 ring-white/55 ring-inset transition-[box-shadow] duration-150",
          "hover:ring-2 hover:ring-primary focus-visible:ring-2 focus-visible:ring-primary",
          dragging && "ring-2 ring-primary",
          disabled ? "cursor-default" : "cursor-move",
        )}
        style={{
          left: `${shown.x * 100}%`,
          top: `${shown.y * 100}%`,
          width: `${shown.width * 100}%`,
          height: `${shown.height * 100}%`,
        }}
      >
        {/* Stretched while dragging, so the box changes at once; drawn sharp again when let go. */}
        <canvas ref={canvas} aria-hidden className="absolute inset-0 size-full" />
        {waiting && (
          <LoaderCircle
            aria-label="Fetching the chat"
            className="absolute top-1.5 right-1.5 size-4 animate-spin text-white/80 drop-shadow"
          />
        )}
        {typeof status === "object" && (
          <p className="absolute inset-x-2 bottom-2 rounded-md bg-black/70 px-2 py-1 text-[11px] leading-snug text-white">
            Couldn’t show the chat: {status.error}
          </p>
        )}
        {!disabled &&
          (["nw", "ne", "sw", "se"] as const).map((corner) => (
            <span
              key={corner}
              aria-hidden
              onPointerDown={grab(corner)}
              className={cn(
                "absolute size-3 rounded-full border-2 border-primary bg-white shadow-sm",
                "opacity-0 transition-opacity duration-150 group-hover/box:opacity-100 group-focus-visible/box:opacity-100",
                dragging && "opacity-100",
                corner === "nw" && "-top-1.5 -left-1.5 cursor-nwse-resize",
                corner === "ne" && "-top-1.5 -right-1.5 cursor-nesw-resize",
                corner === "sw" && "-bottom-1.5 -left-1.5 cursor-nesw-resize",
                corner === "se" && "-right-1.5 -bottom-1.5 cursor-nwse-resize",
              )}
            />
          ))}
      </div>
    </div>
  );
}
