import { motion } from "motion/react";
import type { ReactNode } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";

interface ChipProps {
  selected: boolean;
  onSelect: () => void;
  /** Chips sharing a group share one highlight that slides between them. */
  group: string;
  /** "solid" for primary choices (presets), "soft" for secondary ones (quality). */
  tone?: "solid" | "soft";
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}

/** One option in a single-choice row. The selected highlight glides between chips. */
export function Chip({ selected, onSelect, group, tone = "solid", disabled, className, children }: ChipProps) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        "relative flex shrink-0 items-center gap-1.5 rounded-full font-medium whitespace-nowrap transition-colors duration-150 outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-3.5 [&_svg]:shrink-0",
        tone === "solid" ? "h-8 px-3 text-[13px]" : "h-7 px-2.5 text-xs",
        selected
          ? tone === "solid"
            ? "text-primary-foreground"
            : "text-primary"
          : "text-muted-foreground hover:bg-accent hover:text-foreground",
        className,
      )}
    >
      {selected && (
        <motion.span
          layoutId={`chip-${group}`}
          transition={spring}
          className={cn(
            "absolute inset-0 rounded-full",
            tone === "solid" ? "bg-primary" : "bg-primary-soft",
          )}
        />
      )}
      <span className="relative flex items-center gap-1.5">{children}</span>
    </button>
  );
}
