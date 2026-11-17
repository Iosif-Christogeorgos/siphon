import { motion } from "motion/react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";

/**
 * A small either/or switch: the options sit in one track and the selected
 * one is highlighted, with the highlight gliding between them.
 */
export function Segmented<T extends string | number>({
  label,
  group,
  value,
  options,
  onChange,
  className,
}: {
  /** Read by screen readers. */
  label: string;
  /** Unique per control: the highlight's animation is keyed on it. */
  group: string;
  value: T;
  options: { id: T; label: string }[];
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className={cn("flex shrink-0 rounded-lg bg-accent p-0.5", className)}>
      {options.map((option) => {
        const selected = option.id === value;
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            onClick={() => onChange(option.id)}
            className={cn(
              "relative h-7 flex-1 rounded-md px-2.5 text-xs font-medium whitespace-nowrap outline-none transition-colors duration-150 focus-visible:ring-[3px] focus-visible:ring-ring/40",
              selected ? "text-primary" : "text-muted-foreground hover:text-foreground",
            )}
          >
            {selected && (
              <motion.span
                layoutId={`segmented-${group}`}
                transition={spring}
                className="absolute inset-0 rounded-md bg-primary-soft ring-1 ring-primary/15"
              />
            )}
            <span className="relative tabular-nums">{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}
