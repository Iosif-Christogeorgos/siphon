import { motion } from "motion/react";
import type { ReactNode } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { onRadioKeys } from "./radio-keys";

/**
 * A small either/or switch: the options sit in one track and the selected
 * one is highlighted, with the highlight gliding between them. Arrow keys
 * move between options.
 */
export function Segmented<T extends string | number>({
  label,
  group,
  value,
  options,
  onChange,
  className,
  size = "sm",
}: {
  /** Read by screen readers. */
  label: string;
  /** Unique per control: the highlight's animation is keyed on it. */
  group: string;
  /** null when no option is selected. */
  value: T | null;
  options: { id: T; label: string; icon?: ReactNode }[];
  onChange: (value: T) => void;
  className?: string;
  /** "md" for a primary row, such as the preset groups. */
  size?: "sm" | "md";
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      onKeyDown={onRadioKeys}
      className={cn("flex shrink-0 rounded-lg bg-accent p-0.5", className)}
    >
      {options.map((option) => {
        const selected = option.id === value;
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            // One stop for Tab; the arrow keys move within the row.
            tabIndex={selected || (value === null && option === options[0]) ? 0 : -1}
            onClick={() => onChange(option.id)}
            className={cn(
              "relative flex flex-1 items-center justify-center gap-1.5 rounded-md font-medium whitespace-nowrap outline-none transition-colors duration-150 focus-visible:ring-[3px] focus-visible:ring-ring/40 [&_svg]:size-3.5 [&_svg]:shrink-0",
              size === "md" ? "h-8 px-3 text-[13px]" : "h-7 px-2.5 text-xs",
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
            <span className="relative flex items-center gap-1.5 tabular-nums">
              {option.icon}
              {option.label}
            </span>
          </button>
        );
      })}
    </div>
  );
}
