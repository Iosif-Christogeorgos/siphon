import { motion } from "motion/react";
import type { ReactNode } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { onRadioKeys } from "./radio-keys";
import { Hint } from "./tooltip";

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
  variant = "soft",
}: {
  /** Read by screen readers. */
  label: string;
  /** Unique per control: the highlight's animation is keyed on it. */
  group: string;
  /** null when no option is selected. */
  value: T | null;
  /** `hint` explains an option on hover. A `disabled` option stays in view, its hint saying why. */
  options: { id: T; label: string; icon?: ReactNode; hint?: string; disabled?: boolean }[];
  onChange: (value: T) => void;
  className?: string;
  /** "md" for a primary row, such as the preset groups. */
  size?: "sm" | "md";
  /**
   * "soft": a tinted highlight on a faint track. "tray": keys in a recessed
   * tray, the chosen one raised out of it, as a track's flags are (Settings).
   */
  variant?: "soft" | "tray";
}) {
  const tray = variant === "tray";
  return (
    <div
      role="radiogroup"
      aria-label={label}
      onKeyDown={onRadioKeys}
      className={cn(
        "flex shrink-0",
        tray
          ? "gap-0.5 rounded-[10px] border border-border bg-well p-[3px] shadow-[inset_0_1px_2px_rgb(0_0_0/0.1)]"
          : "rounded-lg bg-accent p-0.5",
        className,
      )}
    >
      {options.map((option) => {
        const selected = option.id === value;
        const button = (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            // Not `disabled`: a disabled button shows no hint, and the hint says why.
            aria-disabled={option.disabled || undefined}
            // One stop for Tab; the arrow keys move within the row.
            tabIndex={selected || (value === null && option === options[0]) ? 0 : -1}
            onClick={() => !option.disabled && onChange(option.id)}
            className={cn(
              "relative flex flex-1 items-center justify-center gap-1.5 font-medium whitespace-nowrap outline-none transition-[color,background-color,transform] duration-150 focus-visible:ring-[3px] focus-visible:ring-ring/40 [&_svg]:size-3.5 [&_svg]:shrink-0",
              tray ? "rounded-[7px]" : "rounded-md",
              size === "md" ? "h-8 px-3 text-[13px]" : "h-7 px-2.5 text-xs",
              option.disabled
                ? "cursor-default text-subtle-foreground opacity-60"
                : selected
                  ? "text-primary"
                  : cn(
                      "text-muted-foreground hover:text-foreground",
                      tray && "hover:bg-accent active:translate-y-px active:scale-[0.98]",
                    ),
            )}
          >
            {selected && (
              <motion.span
                layoutId={`segmented-${group}`}
                transition={spring}
                className={cn(
                  "absolute inset-0",
                  tray
                    ? "rounded-[7px] bg-popover shadow-[var(--card-edge),0_0_0_1px_var(--border),0_1px_2px_rgb(0_0_0/0.16)]"
                    : "rounded-md bg-primary-soft ring-1 ring-primary/15",
                )}
              />
            )}
            <span className="relative flex items-center gap-1.5 tabular-nums">
              {option.icon}
              {option.label}
            </span>
          </button>
        );
        return option.hint ? (
          <Hint key={option.id} label={option.hint} className="max-w-64 text-pretty">
            {button}
          </Hint>
        ) : (
          button
        );
      })}
    </div>
  );
}
