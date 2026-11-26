import { type ReactNode, useState } from "react";
import { cn } from "@/lib/utils";

/**
 * A track's flags (plays first, forced, SDH…) are keys in one recessed tray, like
 * a mixing desk's (2026-11-26, the owner's pick of three prototypes). Grouped, they
 * read as one set of choices about the track rather than loose buttons.
 */
export function FlagTray({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div
      role="group"
      aria-label="Flags"
      className={cn(
        "inline-flex flex-wrap gap-[3px] rounded-xl border border-border bg-well p-[3px] shadow-[inset_0_1px_2px_rgb(0_0_0/0.12)]",
        className,
      )}
    >
      {children}
    </div>
  );
}

/**
 * One flag: a key whose light is on or off. On, it rises out of the tray and its
 * light comes on with a ring rippling out once; it presses down a touch when clicked.
 * "some" is for a season's row whose files differ: the light half lit, the key not raised.
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
  // Each time a click turns it on, the ring is drawn afresh (a new key restarts it).
  const [pops, setPops] = useState(0);
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on === "some" ? "mixed" : on}
      onClick={() => {
        if (on !== true) setPops(pops + 1);
        onClick();
      }}
      className={cn(
        "inline-flex h-[30px] shrink-0 items-center gap-2 rounded-[9px] pr-3 pl-2.5 text-xs transition-[background-color,color,box-shadow,transform] duration-200 ease-out-expo focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none active:translate-y-px active:scale-[0.98]",
        on === true
          ? "bg-card text-foreground shadow-[0_1px_2px_rgb(0_0_0/0.18)]"
          : on === "some"
            ? "text-primary hover:bg-accent"
            : "text-muted-foreground hover:bg-accent hover:text-foreground",
        className,
      )}
    >
      <span
        aria-hidden
        className={cn(
          "relative size-[7px] shrink-0 rounded-full transition-[background-color,box-shadow] duration-300",
          on === true
            ? "bg-primary shadow-[0_0_0_2px_var(--primary-soft),0_0_10px_1px_var(--primary-glow)]"
            : on === "some"
              ? "bg-primary/45"
              : "bg-border-strong",
        )}
      >
        {pops > 0 && on === true && (
          <span key={pops} className="absolute -inset-px animate-ripple rounded-full border-[1.5px] border-primary" />
        )}
      </span>
      {children}
    </button>
  );
}
