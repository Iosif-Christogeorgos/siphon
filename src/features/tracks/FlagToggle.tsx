import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * A flag a track has or hasn't (plays first, forced, SDH…): a chip with a small
 * switch in it, so off reads as off and pointing at it never looks like on.
 * "some" is for a season's row whose files differ: the knob rests in the middle.
 */
export function FlagToggle({
  on,
  onClick,
  children,
  className,
}: {
  on: boolean | "some";
  onClick: () => void;
  children: ReactNode;
  className?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on === "some" ? "mixed" : on}
      onClick={onClick}
      className={cn(
        "group inline-flex h-8 shrink-0 items-center gap-2 rounded-lg border px-2.5 text-xs transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none",
        on === true
          ? "border-primary/45 bg-primary-soft text-primary"
          : on === "some"
            ? "border-primary/30 text-primary hover:border-primary/50"
            : "border-border-strong text-muted-foreground hover:border-primary/45 hover:text-foreground",
        className,
      )}
    >
      <span
        aria-hidden
        className={cn(
          "relative h-3 w-5 shrink-0 rounded-full transition-colors",
          on === true ? "bg-primary" : on === "some" ? "bg-primary/40" : "bg-border-strong group-hover:bg-primary/30",
        )}
      >
        <span
          className={cn(
            "absolute top-0.5 left-0.5 size-2 rounded-full bg-background shadow-sm transition-transform duration-200 ease-out-expo",
            on === true ? "translate-x-2" : on === "some" ? "translate-x-1" : "translate-x-0",
          )}
        />
      </span>
      {children}
    </button>
  );
}
