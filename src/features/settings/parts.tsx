import type { LucideIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";

/** The sections of the Settings panel, in order; its tabs follow them. */
export const SECTIONS = [
  { id: "downloads", tab: "Downloads" },
  { id: "languages", tab: "Languages" },
  { id: "sites", tab: "Sites" },
  { id: "appearance", tab: "Appearance" },
  { id: "sync", tab: "Sync" },
  { id: "updates", tab: "Updates" },
] as const;

export type SectionId = (typeof SECTIONS)[number]["id"];

/**
 * One section of the panel: an icon and a title over its cards. Sections rise
 * into place one after another as the panel opens.
 */
export function Section({
  id,
  index,
  icon: Icon,
  title,
  hint,
  children,
}: {
  id: SectionId;
  /** Its place in the panel, for the order it rises in. */
  index: number;
  icon: LucideIcon;
  title: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <motion.section
      id={`settings-${id}`}
      data-section={id}
      aria-labelledby={`settings-${id}-title`}
      initial={{ opacity: 0, y: 14, filter: "blur(4px)" }}
      animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
      transition={{ ...spring, delay: 0.06 + index * 0.05 }}
    >
      <div className="mb-2.5 px-1">
        <h3 id={`settings-${id}-title`} className="flex items-center gap-2 text-[13px] font-semibold tracking-tight">
          <Icon className="size-3.5 text-primary" strokeWidth={2} />
          {title}
        </h3>
        {hint && <p className="mt-0.5 text-xs text-pretty text-subtle-foreground">{hint}</p>}
      </div>
      <div className="space-y-2.5">{children}</div>
    </motion.section>
  );
}

/** A raised card holding settings rows, separated by inset hairlines. */
export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn("overflow-hidden rounded-2xl border bg-card shadow-[var(--card-edge),0_1px_2px_color-mix(in_srgb,var(--shadow-color)_6%,transparent)]", className)}>
      {children}
    </div>
  );
}

/**
 * One setting: its name, with a small control (`control`) on the same line,
 * a short explanation under them at full width, and a wide control
 * (`children`) below that.
 */
export function Row({
  title,
  hint,
  control,
  children,
  className,
}: {
  title: ReactNode;
  hint?: ReactNode;
  control?: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "relative px-4 py-3.5",
        // Inset hairlines between rows, not edge to edge.
        "not-first:before:absolute not-first:before:inset-x-4 not-first:before:top-0 not-first:before:h-px not-first:before:bg-border",
        className,
      )}
    >
      <div className="flex min-h-7 items-center justify-between gap-3">
        <div className="min-w-0 truncate text-[13px] font-medium">{title}</div>
        {control && <div className="flex min-w-0 shrink-0 justify-end">{control}</div>}
      </div>
      {hint && <p className="mt-0.5 text-xs text-pretty text-muted-foreground">{hint}</p>}
      {children && <div className="mt-3">{children}</div>}
    </div>
  );
}

/** Opens and closes by growing and shrinking in place, so what is below moves rather than jumps. */
export function Reveal({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <AnimatePresence initial={false}>
      {open && (
        <motion.div
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={{ ...spring, opacity: { duration: 0.16 } }}
          // Room for a focused field's glow, which overflow would otherwise clip.
          className="-mx-1.5 overflow-hidden px-1.5"
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/** A text field in the panel, lit the way the link field is when focused. */
export const fieldClass =
  "h-9 rounded-lg border border-border-strong bg-background/60 px-3 text-[13px] outline-none transition-[border-color,box-shadow] duration-300 ease-out-expo placeholder:text-subtle-foreground focus:border-primary/85 focus:shadow-[0_0_0_1px_color-mix(in_oklab,var(--primary)_22%,transparent),0_0_24px_-6px_var(--primary-glow)]";
