import { Check, Clock, Info, RefreshCw, Scissors, Shrink, SlidersHorizontal, TriangleAlert, Zap } from "lucide-react";
import {
  AnimatePresence,
  motion,
  useMotionValue,
  useReducedMotion,
  useSpring,
  useTransform,
} from "motion/react";
import { type ReactNode, useEffect } from "react";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatBytesRange } from "@/lib/format";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { Badge, LiveStatus, PlanSummary } from "@/types";
import { NoticeStack } from "./NoticeStack";

const BADGES: Record<Badge, { icon: typeof Zap; label: string; accent: boolean }> = {
  lossless: { icon: Zap, label: "No quality loss", accent: true },
  converts: { icon: RefreshCw, label: "Converts", accent: false },
  size_optimized: { icon: Shrink, label: "Size-optimized", accent: true },
  may_not_play: { icon: TriangleAlert, label: "May not play everywhere", accent: false },
};

/** 42 → "~42 s", 150 → "~3 min", 4000 → "~1 h 7 min". */
export function formatConvertTime(seconds: number): string {
  if (seconds < 60) return `~${Math.max(5, Math.round(seconds / 5) * 5)} s`;
  if (seconds < 3600) return `~${Math.round(seconds / 60)} min`;
  return `~${Math.floor(seconds / 3600)} h ${Math.round((seconds % 3600) / 60)} min`;
}

/** Says, in plain language, exactly what pressing Download will produce. */
export function PlanCard({
  plan,
  clips,
  customizing,
  onCustomize,
  customizable = true,
  onTrim,
  perHour = false,
  subtitles,
  measuring = false,
}: {
  plan: PlanSummary | undefined;
  clips: number;
  /** The custom settings are open. */
  customizing: boolean;
  onCustomize: () => void;
  /** Offers Customize here; a file has it beside the presets instead. */
  customizable?: boolean;
  /** Opens the trimmer; offered when the plan is blocked for being too long. */
  onTrim?: () => void;
  /** A recording: sizes are for each hour of it. */
  perHour?: boolean;
  /** What the plan does with subtitles, in a few words: "English subtitles". */
  subtitles?: string | null;
  /** The size is being measured on the video, for a closer figure. */
  measuring?: boolean;
}) {
  if (!plan) {
    return (
      <div aria-hidden className="flex gap-3 px-1">
        <div className="skeleton size-9 shrink-0 rounded-xl" />
        <div className="flex-1 space-y-2 py-0.5">
          <div className="skeleton h-3.5 w-2/5" />
          <div className="skeleton h-3 w-4/5" />
        </div>
      </div>
    );
  }

  const customize = customizable && (
    <button
      type="button"
      onClick={onCustomize}
      aria-pressed={customizing}
      className="-mr-1 flex items-center gap-1 rounded-md px-1 text-xs text-subtle-foreground transition-colors hover:text-primary focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
    >
      <SlidersHorizontal className="size-3" />
      {customizing ? "Back to presets" : "Customize"}
    </button>
  );

  // A file that already is what was asked for: said plainly, and nothing to make.
  if (plan.nothing) {
    return (
      <div className="flex items-center gap-3 px-1 py-1.5">
        <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary-soft text-primary">
          <Check className="size-4" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{plan.nothing.replace(/\s*Nothing to convert\.$/, "")}</p>
          <p className="truncate text-xs text-muted-foreground">{plan.headline}, as it is</p>
        </div>
        <div className="flex shrink-0 flex-col items-end self-end">{customize}</div>
      </div>
    );
  }

  const badge = BADGES[plan.badge];
  const Icon = badge.icon;
  // How long converting (or, for a copied clip, cutting) takes. Share's badge says enough on its own.
  const time =
    plan.convert_seconds !== null && plan.badge !== "size_optimized" ? formatConvertTime(plan.convert_seconds) : null;
  // Only warnings stay in view, folded into one stack; the explanation and notes for the curious wait behind the ⓘ.
  const notes = plan.hints.filter((hint) => hint.level === "info");
  const warnings = plan.hints.filter((hint) => hint.level === "warning");

  return (
    <div className="space-y-2.5 py-1.5">
      <div className="flex items-center gap-3 px-1">
        <div
          className={cn(
            "grid size-9 shrink-0 place-items-center rounded-xl",
            badge.accent ? "bg-primary-soft text-primary" : "bg-secondary text-muted-foreground",
          )}
        >
          <Icon className="size-4" />
        </div>

        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-1">
            <p className="truncate font-medium">{plan.headline}</p>
            <Hint
              label={
                <span className="block space-y-1.5">
                  <span className="block">{plan.detail}</span>
                  {notes.map((note) => (
                    <span key={note.text} className="block text-muted-foreground">
                      {note.text}
                    </span>
                  ))}
                </span>
              }
              className="max-w-80 px-3 py-2 leading-relaxed text-pretty"
            >
              <button
                type="button"
                aria-label="About this file"
                className={cn(
                  "grid size-6 shrink-0 place-items-center rounded-full transition-colors hover:bg-accent hover:text-foreground",
                  notes.length > 0 ? "text-primary" : "text-subtle-foreground",
                )}
              >
                <Info className="size-3.5" />
              </button>
            </Hint>
          </div>
          <p className="text-xs text-muted-foreground">
            <span className={cn(badge.accent && "text-primary")}>{badge.label}</span>
            {time && ` · ${time}`}
            {plan.chat && " · Chat drawn in"}
            {subtitles && ` · ${subtitles}`}
          </p>
        </div>

        <div className="flex shrink-0 flex-col items-end">
          {plan.estimated_size != null ? (
            <span className="text-[15px] font-medium tabular-nums">
              {plan.size_range ? (
                // A conversion's size depends on the picture: say how far it can go, until it's measured.
                <Hint
                  label={
                    measuring
                      ? `Most likely about ${formatBytes(plan.estimated_size)}. Measuring this video for a closer figure…`
                      : `Most likely about ${formatBytes(plan.estimated_size)}; it depends on the picture.`
                  }
                >
                  <span className={cn(measuring && "animate-pulse motion-reduce:animate-none")}>
                    {formatBytesRange(plan.size_range[0], plan.size_range[1])}
                  </span>
                </Hint>
              ) : plan.measured ? (
                <Hint label="Measured on this video: within about a tenth.">
                  <span>
                    <span className="text-muted-foreground">≈ </span>
                    <AnimatedBytes bytes={plan.estimated_size} />
                  </span>
                </Hint>
              ) : (
                <>
                  <span className="text-muted-foreground">≈ </span>
                  <AnimatedBytes bytes={plan.estimated_size} />
                </>
              )}
              {clips > 1 && <span className="text-xs font-normal text-muted-foreground"> each</span>}
              {perHour && <span className="text-xs font-normal text-muted-foreground"> per hour</span>}
            </span>
          ) : (
            <span className="h-[22px]" />
          )}
          {customize}
        </div>
      </div>

      <AnimatePresence initial={false}>
        {plan.blocked && (
          <Note key="blocked" warning>
            <span className="flex-1">{plan.blocked}</span>
            {onTrim && (
              <Button size="sm" variant="secondary" className="-my-1 h-7" onClick={onTrim}>
                <Scissors /> Trim
              </Button>
            )}
          </Note>
        )}
        {warnings.length > 0 && <NoticeStack key="warnings" notices={warnings} />}
      </AnimatePresence>
    </div>
  );
}

/**
 * In place of the plan for a stream that hasn't started: what happens next.
 * The quality is planned once it is on air, from what it offers then.
 */
export function LiveWaitCard({ status }: { status: Exclude<LiveStatus, "live"> }) {
  return (
    <div className="flex items-center gap-3 px-1 py-1.5">
      <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-secondary text-muted-foreground">
        <Clock className="size-4" />
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium">
          {status === "upcoming" ? "Records once the stream starts" : "Records once the channel goes live"}
        </p>
        <p className="text-xs text-pretty text-muted-foreground">
          Siphon keeps checking, starts recording by itself and lets you know. The quality is picked then.
        </p>
      </div>
    </div>
  );
}

export function Note({ warning, children }: { warning: boolean; children: ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: "auto" }}
      exit={{ opacity: 0, height: 0 }}
      transition={spring}
      className="overflow-hidden"
    >
      <div
        className={cn(
          "flex items-start gap-2 rounded-xl px-3 py-2 text-[13px] text-pretty",
          warning ? "bg-destructive-soft text-foreground" : "bg-secondary text-muted-foreground",
        )}
      >
        {warning ? (
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" />
        ) : (
          <Info className="mt-0.5 size-3.5 shrink-0" />
        )}
        {children}
      </div>
    </motion.div>
  );
}

/** A size that glides to its new value instead of jumping (unless motion is reduced). */
function AnimatedBytes({ bytes }: { bytes: number }) {
  const reduce = useReducedMotion();
  const target = useMotionValue(bytes);
  const smooth = useSpring(target, { stiffness: 140, damping: 24 });
  const text = useTransform(reduce ? target : smooth, (value) => formatBytes(value));
  useEffect(() => target.set(bytes), [target, bytes]);
  return <motion.span>{text}</motion.span>;
}
