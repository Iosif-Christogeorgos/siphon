import { Info, RefreshCw, Scissors, Shrink, SlidersHorizontal, TriangleAlert, Zap } from "lucide-react";
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
import type { Badge, PlanSummary } from "@/types";

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
  onTrim,
}: {
  plan: PlanSummary | undefined;
  clips: number;
  /** The custom settings are open. */
  customizing: boolean;
  onCustomize: () => void;
  /** Opens the trimmer; offered when the plan is blocked for being too long. */
  onTrim?: () => void;
}) {
  if (!plan) {
    return (
      <div aria-hidden className="flex gap-3 px-1 py-1">
        <div className="skeleton size-8 shrink-0 rounded-lg" />
        <div className="flex-1 space-y-2 py-0.5">
          <div className="skeleton h-3.5 w-2/5" />
          <div className="skeleton h-3 w-4/5" />
        </div>
      </div>
    );
  }

  const badge = BADGES[plan.badge];
  const Icon = badge.icon;
  const badgeText = [
    badge.label,
    // How long converting (or, for a copied clip, cutting) takes. Share's
    // badge says enough on its own.
    plan.convert_seconds !== null && plan.badge !== "size_optimized" && formatConvertTime(plan.convert_seconds),
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <div className="space-y-2.5 px-1 py-1">
      <div className="flex gap-3">
        <div
          className={cn(
            "grid size-8 shrink-0 place-items-center rounded-lg",
            badge.accent ? "bg-primary-soft text-primary" : "bg-secondary text-muted-foreground",
          )}
        >
          <Icon className="size-4" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <p className="truncate font-medium">{plan.headline}</p>
            {plan.estimated_size != null && (
              <span className="shrink-0 text-[13px] text-muted-foreground tabular-nums">
                {plan.size_range ? (
                  // A conversion's size depends on the picture: say how far it can go.
                  <Hint label={`Most likely about ${formatBytes(plan.estimated_size)}; it depends on the picture.`}>
                    <span>{formatBytesRange(plan.size_range[0], plan.size_range[1])}</span>
                  </Hint>
                ) : (
                  <>
                    ≈ <AnimatedBytes bytes={plan.estimated_size} />
                  </>
                )}
                {clips > 1 && " each"}
              </span>
            )}
          </div>
          <p className="text-[13px] text-pretty text-muted-foreground">{plan.detail}</p>
          <div className="mt-1.5 flex items-center justify-between gap-3">
            <span
              className={cn(
                "rounded-md px-1.5 py-0.5 text-[11px] font-medium",
                badge.accent ? "bg-primary-soft text-primary" : "bg-secondary text-muted-foreground",
              )}
            >
              {badgeText}
            </span>
            <button
              type="button"
              onClick={onCustomize}
              aria-pressed={customizing}
              className="flex items-center gap-1 rounded-md px-1 text-xs text-subtle-foreground transition-colors hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
            >
              <SlidersHorizontal className="size-3" />
              {customizing ? "Back to presets" : "Customize"}
            </button>
          </div>
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
        {plan.hints.map((hint) => (
          <Note key={hint.text} warning={hint.level === "warning"}>
            {hint.text}
          </Note>
        ))}
      </AnimatePresence>
    </div>
  );
}

function Note({ warning, children }: { warning: boolean; children: ReactNode }) {
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
