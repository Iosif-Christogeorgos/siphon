import { Info, RefreshCw, TriangleAlert, Zap } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { formatBytes } from "@/lib/format";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { PlanSummary } from "@/types";

/** Says, in plain language, exactly what pressing Download will produce. */
export function PlanCard({ plan }: { plan: PlanSummary | undefined }) {
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

  const Icon = plan.converts ? RefreshCw : Zap;
  return (
    <div className="space-y-2.5 px-1 py-1">
      <div className="flex gap-3">
        <div
          className={cn(
            "grid size-8 shrink-0 place-items-center rounded-lg",
            plan.converts ? "bg-secondary text-muted-foreground" : "bg-primary-soft text-primary",
          )}
        >
          <Icon className="size-4" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <p className="truncate font-medium">{plan.headline}</p>
            {plan.estimated_size != null && (
              <span className="shrink-0 text-[13px] text-muted-foreground tabular-nums">
                ≈ {formatBytes(plan.estimated_size)}
              </span>
            )}
          </div>
          <p className="text-[13px] text-pretty text-muted-foreground">{plan.detail}</p>
        </div>
      </div>

      <AnimatePresence initial={false}>
        {plan.hints.map((hint) => (
          <motion.div
            key={hint.text}
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={spring}
            className="overflow-hidden"
          >
            <p
              className={cn(
                "flex gap-2 rounded-xl px-3 py-2 text-[13px] text-pretty",
                hint.level === "warning"
                  ? "bg-destructive-soft text-foreground"
                  : "bg-secondary text-muted-foreground",
              )}
            >
              {hint.level === "warning" ? (
                <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-destructive" />
              ) : (
                <Info className="mt-0.5 size-3.5 shrink-0" />
              )}
              {hint.text}
            </p>
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
