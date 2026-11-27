import { ArrowDown, Gauge } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Hint } from "@/components/ui/tooltip";
import { formatLimit, formatSpeed } from "@/lib/format";
import { spring } from "@/lib/motion";
import { useSettings } from "@/stores/settings";
import { useSpeed } from "@/stores/speed";
import { useUi } from "@/stores/ui";

/**
 * In the title bar: how fast everything is downloading right now, and, while
 * a speed limit is set, a quiet reminder of it that opens Settings to change it.
 */
export function SpeedIndicator() {
  const total = useSpeed((state) => state.total);
  const limit = useSettings().data?.speed_limit ?? null;
  const openPanel = useUi((state) => state.openPanel);

  return (
    <div className="mr-1 flex items-center gap-1 text-xs">
      <AnimatePresence initial={false}>
        {total > 0 && (
          <motion.span
            key="speed"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            // Not a control: drags on it still move the window.
            className="pointer-events-none flex items-center gap-0.5 px-1 text-muted-foreground tabular-nums"
            aria-label={`Downloading at ${formatSpeed(total)}`}
          >
            <ArrowDown className="size-3" strokeWidth={2} />
            {formatSpeed(total)}
          </motion.span>
        )}
        {limit !== null && (
          <motion.span key="limit" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={spring}>
            <Hint label="All downloads together. Click to change or remove the limit.">
              <button
                type="button"
                onClick={() => openPanel("settings", "downloads")}
                className="flex h-6 items-center gap-1 rounded-full border border-border-strong px-2 text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground"
              >
                <Gauge className="size-3" strokeWidth={2} />
                Limited to {formatLimit(limit)}
              </button>
            </Hint>
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}
