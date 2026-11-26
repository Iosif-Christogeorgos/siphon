import { ChevronDown, TriangleAlert } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import type { Hint } from "@/types";

/** Folded, the top card is one row; each card under it peeks out below, two at most. */
const ROW = 40;
const PEEK = 7;
const GAP = 6;
/** Softer than the app's `spring`, with a little give at the end, as in the prototype the owner chose. */
const FOLD = { type: "spring", stiffness: 300, damping: 26 } as const;

/**
 * A plan's warnings, folded into one row with the others peeking out under
 * it: their short titles and a count. A click fans the cards out in place to
 * say each in full, and folds them again. Not on hover: passing the pointer
 * over it on the way to the button opened it and pushed the button away.
 */
export function NoticeStack({ notices }: { notices: Hint[] }) {
  const reduce = useReducedMotion();
  const [open, setOpen] = useState(false);

  // Each card's height when open, measured from its content, which wraps with the window's width.
  // The notices are a new array on every render: what they say is what changes.
  const signature = notices.map((notice) => notice.text).join("\n");
  const contents = useRef(new Map<string, HTMLDivElement>());
  const [heights, setHeights] = useState<number[]>([]);
  useLayoutEffect(() => {
    const els = signature.split("\n").map((text) => contents.current.get(text));
    const measure = () => {
      const next = els.map((el) => el?.offsetHeight ?? ROW - 2);
      setHeights((last) => (last.length === next.length && last.every((h, i) => h === next[i]) ? last : next));
    };
    measure();
    const observer = new ResizeObserver(measure);
    for (const el of els) if (el) observer.observe(el);
    return () => observer.disconnect();
  }, [signature]);

  // Another choice that brings other warnings: a band of light crosses the top card once.
  const [shine, setShine] = useState(0);
  const seen = useRef(signature);
  useEffect(() => {
    if (seen.current === signature) return;
    seen.current = signature;
    setShine((count) => count + 1);
  }, [signature]);

  // Open, each card under the last (heights include the border); folded, each a little lower and narrower.
  const full = notices.map((_, i) => (heights[i] ?? ROW - 2) + 2);
  const spots = notices.map((_, i) => {
    if (open) {
      const y = full.slice(0, i).reduce((sum, h) => sum + h + GAP, 0);
      return { y, height: full[i], scale: 1, opacity: 1 };
    }
    const depth = Math.min(i, 2);
    return { y: depth * PEEK, height: ROW, scale: 1 - depth * 0.05, opacity: i > 2 ? 0 : 1 };
  });
  const height = open
    ? full.reduce((sum, h) => sum + h, 0) + GAP * (notices.length - 1)
    : ROW + Math.min(notices.length - 1, 2) * PEEK;
  const summary = notices.map((notice) => notice.title ?? notice.text).join(" · ");

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height }}
      exit={{ opacity: 0, height: 0 }}
      transition={reduce ? { duration: 0 } : FOLD}
      className="relative overflow-hidden"
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) setOpen(false);
      }}
    >
      <AnimatePresence initial={false}>
        {notices.map((notice, i) => {
          const front = i === 0;
          const behind = !open && !front;
          const spot = spots[i];
          const title = notice.title ?? notice.text;
          const header = (
            <>
              <TriangleAlert className="size-[15px] shrink-0 text-caution-foreground" />
              <span className="grid min-w-0 flex-1 text-[13.5px] font-medium">
                {front && (
                  <span
                    className={cn("col-start-1 row-start-1 truncate transition-opacity duration-200", open && "opacity-0")}
                  >
                    {summary}
                  </span>
                )}
                <span
                  className={cn(
                    "col-start-1 row-start-1 truncate transition-opacity duration-200",
                    front && !open && "opacity-0",
                  )}
                >
                  {title}
                </span>
              </span>
              {front && notices.length > 1 && (
                <motion.span
                  animate={{ opacity: open ? 0 : 1, scale: open ? 0.6 : 1 }}
                  transition={reduce ? { duration: 0 } : FOLD}
                  className="grid h-5 min-w-5 shrink-0 place-items-center rounded-full bg-caution px-1.5 text-[11.5px] font-bold text-background tabular-nums"
                >
                  {notices.length}
                </motion.span>
              )}
              {front && (
                <motion.span
                  animate={{ rotate: open ? 180 : 0 }}
                  transition={reduce ? { duration: 0 } : FOLD}
                  className="grid shrink-0 place-items-center text-muted-foreground"
                >
                  <ChevronDown className="size-4" />
                </motion.span>
              )}
            </>
          );
          const row = "flex h-[38px] w-full items-center gap-2.5 px-3 text-left";
          return (
            <motion.div
              key={notice.text}
              initial={{ ...spot, opacity: 0 }}
              animate={spot}
              exit={{ opacity: 0, transition: { duration: 0.12 } }}
              transition={reduce ? { duration: 0 } : { ...FOLD, delay: (open ? i : notices.length - 1 - i) * 0.025 }}
              style={{ zIndex: notices.length - i, transformOrigin: "50% 100%" }}
              className={cn(
                "absolute inset-x-0 top-0 overflow-hidden rounded-xl border border-caution-edge transition-colors duration-300",
                behind ? "bg-notice-behind" : "bg-notice",
              )}
            >
              {front && shine > 0 && (
                <span
                  key={shine}
                  aria-hidden
                  className="pointer-events-none absolute inset-0 animate-notice-shine bg-[linear-gradient(105deg,transparent_35%,var(--caution-glow)_50%,transparent_65%)] bg-size-[250%_100%] bg-no-repeat motion-reduce:hidden"
                />
              )}
              <div
                ref={(el) => {
                  if (el) contents.current.set(notice.text, el);
                  else contents.current.delete(notice.text);
                }}
                className={cn("transition-opacity duration-200", behind && "opacity-0")}
              >
                {front ? (
                  <button
                    type="button"
                    aria-expanded={open}
                    onClick={() => setOpen((was) => !was)}
                    className={cn(
                      row,
                      "rounded-xl focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none focus-visible:ring-inset",
                    )}
                  >
                    {header}
                  </button>
                ) : (
                  <div className={row}>{header}</div>
                )}
                <p
                  className={cn(
                    "pr-3 pb-3 pl-[37px] text-[13px] leading-[1.45] text-pretty text-foreground/85 transition-opacity",
                    open ? "opacity-100 delay-100 duration-300" : "opacity-0 duration-150",
                  )}
                >
                  {notice.text}
                </p>
              </div>
            </motion.div>
          );
        })}
      </AnimatePresence>
    </motion.div>
  );
}
