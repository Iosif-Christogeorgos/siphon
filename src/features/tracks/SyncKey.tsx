import { AudioLines, Check, Clock3, Lock, Undo2, X } from "lucide-react";
import { motion } from "motion/react";
import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Hint } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export type SyncKeyState = "idle" | "locked" | "waiting" | "running" | "done";

const ICONS: Record<SyncKeyState, ReactNode> = {
  idle: <AudioLines className="sync-bars size-3.5" />,
  locked: <Lock className="size-3.5" />,
  waiting: <Clock3 className="size-3.5 animate-breathe" />,
  running: <AudioLines className="sync-bars sync-bars-live size-3.5 text-primary" />,
  done: <Check className="sync-check size-3.5 text-primary" strokeWidth={2.5} />,
};

/**
 * **Sync to speech** as a key in a tray of its own, beside a track's flags
 * (2026-11-26, the owner's pick of three prototypes). Pressed, it widens into its
 * own progress, filling as a download does, with a key to stop beside it; done,
 * it says Synced, its check drawn with a ring rippling out, and an undo beside it.
 * The tray follows what it holds in width, so each change of label glides.
 */
export function SyncKey({
  state,
  label,
  aside,
  share = 0,
  hint,
  onPress,
  disabled = false,
  side,
  small = false,
  className,
}: {
  state: SyncKeyState;
  label: string;
  /** After the label: how far it has got ("45%"), or how many are ahead of it. */
  aside?: string | null;
  /** How much is done, 0 to 1, while it runs. */
  share?: number;
  /** What it does, or what it is doing: shown when pointed at. */
  hint: ReactNode;
  onPress?: () => void;
  disabled?: boolean;
  /** A second key beside it: stop while it waits or runs, undo once done. */
  side?: { label: string; icon: "stop" | "undo"; onClick: () => void } | null;
  /** A season's row is a little shorter than a track's details. */
  small?: boolean;
  className?: string;
}) {
  // The tray takes the width of what it holds, animated (a width can't be
  // transitioned to "auto", so it is measured).
  const inner = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  useLayoutEffect(() => {
    const element = inner.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setWidth(element.offsetWidth));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // A ring ripples out from the check when a sync has just finished, not when a
  // track already synced is opened.
  const [pops, setPops] = useState(0);
  const was = useRef(state);
  useEffect(() => {
    if (state === "done" && was.current === "running") setPops((count) => count + 1);
    was.current = state;
  }, [state]);

  const height = small ? "h-[26px]" : "h-[30px]";
  const pressable = (state === "idle" || state === "locked") && !disabled;

  return (
    <motion.div
      initial={false}
      animate={{ width: width ?? "auto" }}
      transition={{ duration: 0.55, ease: [0.32, 0.72, 0, 1] }}
      className={cn(
        "shrink-0 overflow-hidden rounded-xl bg-well shadow-[inset_0_0_0_1px_var(--border),inset_0_1px_2px_rgb(0_0_0/0.12)]",
        className,
      )}
    >
      {/* 4 px in from an inset edge, as the flags' keys are 3 px inside a 1 px border. */}
      <div ref={inner} className="flex w-max items-center gap-[3px] p-1">
        <Hint label={hint} className="max-w-72 text-pretty">
          <button
            type="button"
            aria-disabled={!pressable}
            onClick={pressable ? onPress : undefined}
            className={cn(
              "sync-key relative inline-flex items-center overflow-hidden rounded-[9px] pr-3 pl-2.5 text-xs font-medium whitespace-nowrap transition-[background-color,color,box-shadow,transform] duration-200 ease-out-expo focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
              height,
              pressable && "active:translate-y-px active:scale-[0.98]",
              state === "idle" &&
                "bg-primary-soft text-primary shadow-[0_1px_2px_rgb(0_0_0/0.18),inset_0_0_0_1px_color-mix(in_oklab,var(--primary)_22%,transparent)] hover:bg-primary/18",
              state === "locked" && "text-muted-foreground hover:bg-accent hover:text-foreground",
              (state === "running" || state === "waiting" || state === "done") &&
                "cursor-default bg-card shadow-[0_1px_2px_rgb(0_0_0/0.18)]",
              state === "waiting" ? "text-muted-foreground" : state !== "idle" && state !== "locked" && "text-foreground",
              disabled && "cursor-default opacity-60 hover:bg-transparent",
            )}
          >
            {state === "running" && (
              // The download's liquid: deepening toward its front, a band of light drifting
              // through it, and a bright edge breathing.
              <span
                aria-hidden
                className="absolute inset-y-0 left-0 transition-[width] duration-500 ease-out"
                style={{ width: `${Math.min(1, Math.max(0, share)) * 100}%` }}
              >
                <span className="absolute inset-0 bg-linear-to-r from-primary/5 to-primary/24" />
                <span className="absolute inset-0 animate-sheen bg-[linear-gradient(100deg,transparent_30%,color-mix(in_oklab,var(--primary)_20%,transparent)_50%,transparent_70%)] bg-size-[250%_100%] motion-reduce:hidden" />
                <span className="absolute inset-y-1 right-0 w-0.5 animate-breathe rounded-full bg-primary shadow-[0_0_8px_1px_var(--primary-glow)] motion-reduce:animate-none" />
              </span>
            )}
            <span className="relative flex items-center gap-2">
              <span className="relative grid size-3.5 shrink-0 place-items-center">
                {ICONS[state]}
                {pops > 0 && state === "done" && (
                  <span key={pops} className="absolute -inset-px animate-ripple rounded-full border-[1.5px] border-primary" />
                )}
              </span>
              {label}
              {aside && (
                <span className="min-w-[3.4ch] text-right font-medium text-muted-foreground tabular-nums">{aside}</span>
              )}
            </span>
          </button>
        </Hint>
        {side && (
          <Hint label={side.label}>
            <button
              type="button"
              aria-label={side.label}
              onClick={side.onClick}
              className={cn(
                "grid shrink-0 animate-in place-items-center rounded-[9px] text-subtle-foreground transition-colors duration-200 fade-in-0 hover:bg-accent hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
                small ? "size-[26px]" : "size-[30px]",
              )}
            >
              {side.icon === "stop" ? <X className="size-3.5" /> : <Undo2 className="size-3.5" />}
            </button>
          </Hint>
        )}
      </div>
    </motion.div>
  );
}
