import { ArrowDownToLine, Captions, KeyRound, Languages, type LucideIcon, Palette, RefreshCw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { SettingsSection } from "@/stores/ui";

/** Settings' pages, each with its icon and what it is about in a line. */
export const SECTIONS: Record<SettingsSection, { title: string; icon: LucideIcon; description: string }> = {
  downloads: {
    title: "Downloads",
    icon: ArrowDownToLine,
    description: "Where files go, what they’re called, and how fast they come.",
  },
  sites: {
    title: "Signed-in sites",
    icon: KeyRound,
    description: "Videos download without your account unless they need it.",
  },
  languages: {
    title: "Languages",
    icon: Languages,
    description: "The languages you watch in, best first.",
  },
  sync: {
    title: "Subtitle sync",
    icon: Captions,
    description: "Subtitles in time with the speech, worked out on your graphics card.",
  },
  appearance: {
    title: "Appearance",
    icon: Palette,
    description: "How Siphon looks.",
  },
  updates: {
    title: "Updates",
    icon: RefreshCw,
    description: "yt-dlp, FFmpeg and Deno keep themselves current. Siphon asks before it updates itself.",
  },
};

/** The sidebar's groups, by what each page is for. */
export const GROUPS: { title: string; sections: SettingsSection[] }[] = [
  { title: "Downloading", sections: ["downloads", "sites"] },
  { title: "Your files", sections: ["languages", "sync"] },
  { title: "Siphon", sections: ["appearance", "updates"] },
];

/** The pages in the sidebar's order, for which way a change of page moves. */
export const ORDER = GROUPS.flatMap((group) => group.sections);

/** A raised card holding settings rows, separated by inset hairlines. */
export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div
      className={cn(
        "overflow-hidden rounded-2xl border bg-card shadow-[var(--card-edge),0_1px_2px_color-mix(in_srgb,var(--shadow-color)_6%,transparent)]",
        className,
      )}
    >
      {children}
    </div>
  );
}

/**
 * One setting: its name, with a small control (`control`) on the same line,
 * a short explanation under the name, and a wide control (`children`). While
 * the page is narrow the wide control goes under the explanation; once it has
 * room (`.settings-row` in index.css) it goes beside the name and explanation,
 * so each setting reads across. `stacked` keeps it under them all the same,
 * for a row whose wide part is text to read rather than a control.
 */
export function Row({
  title,
  hint,
  control,
  children,
  stacked = false,
  className,
}: {
  title: ReactNode;
  hint?: ReactNode;
  control?: ReactNode;
  children?: ReactNode;
  stacked?: boolean;
  className?: string;
}) {
  return (
    <div
      data-body-only={(children && !control && !stacked) || undefined}
      data-stacked={stacked || undefined}
      className={cn(
        "settings-row relative px-4 py-4 @min-[34rem]/settings:px-5 @min-[34rem]/settings:py-[18px]",
        // Inset hairlines between rows, not edge to edge.
        "not-first:before:absolute not-first:before:inset-x-4 not-first:before:top-0 not-first:before:h-px not-first:before:bg-border @min-[34rem]/settings:not-first:before:inset-x-5",
        className,
      )}
    >
      <div className="flex min-h-7 min-w-0 items-center truncate text-[13px] font-medium tracking-[-0.003em] [grid-area:title]">
        {title}
      </div>
      {control && <div className="flex min-w-0 items-center justify-self-end [grid-area:control]">{control}</div>}
      {hint && <p className="mt-0.5 text-xs leading-normal text-pretty text-muted-foreground [grid-area:hint]">{hint}</p>}
      {children && (
        <div
          className={cn(
            "mt-3.5 min-w-0 self-start [grid-area:body]",
            !stacked && (control ? "@min-[34rem]/settings:mt-2.5" : "@min-[34rem]/settings:mt-0"),
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}

/**
 * A state in a word, after a small light: green and glowing when it works,
 * red when it doesn't, unlit when it isn't there yet; and what else is worth
 * knowing beside it, quieter.
 */
export function Status({
  tone,
  label,
  children,
}: {
  tone: "on" | "off" | "error";
  label: ReactNode;
  children?: ReactNode;
}) {
  return (
    <p className="flex min-w-0 items-start gap-1.5 text-xs leading-normal text-muted-foreground">
      <span
        aria-hidden
        className={cn(
          "mt-[6px] size-1.5 shrink-0 rounded-full",
          tone === "on" && "bg-success shadow-[0_0_6px_var(--success)]",
          tone === "error" && "bg-destructive",
          tone === "off" && "bg-border-strong ring-1 ring-border-strong",
        )}
      />
      <span className="min-w-0 text-pretty">
        <span className={cn("font-medium", tone === "error" ? "text-destructive" : "text-foreground/90")}>{label}</span>
        {children && (
          <>
            <span aria-hidden className="text-subtle-foreground">
              {" · "}
            </span>
            {children}
          </>
        )}
      </span>
    </p>
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

/** A text field in Settings, lit the way the link field is when focused. */
export const fieldClass =
  "h-9 rounded-lg border border-border-strong bg-background/60 px-3 text-[13px] outline-none transition-[border-color,box-shadow,background-color] duration-300 ease-out-expo placeholder:text-subtle-foreground hover:border-foreground/25 focus:border-primary/85 focus:bg-background/80 focus:shadow-[0_0_0_1px_color-mix(in_oklab,var(--primary)_22%,transparent),0_0_24px_-6px_var(--primary-glow)]";

/** A recessed strip showing what a choice makes (a file's name): read, not clicked. */
export const wellClass = "rounded-lg bg-well shadow-[inset_0_0_0_1px_var(--border),inset_0_1px_2px_rgb(0_0_0/0.08)]";
