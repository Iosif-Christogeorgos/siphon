import { ChevronRight } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type KeyboardEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { onRadioKeys } from "./radio-keys";

export interface FacetOption<T> {
  id: T;
  label: ReactNode;
  /** What screen readers say, when `label` isn't plain text. */
  text?: string;
  icon?: ReactNode;
  /** Small print for this option (a size, a limit), shown at the end while it is pointed at. */
  detail?: string;
}

/** Waits this long after the pointer leaves, so crossing a gap doesn't snap it shut. */
const CLOSE_DELAY = 160;

/**
 * One choice, folded down to its current value: "Quality  Best ›".
 * Pointing at it (or tabbing into it) unfolds the other options in place,
 * floating over its neighbours so nothing around it moves. An aqua mark
 * glides to the selected option, a soft highlight follows the pointer, and
 * the pointed-at option's small print shows at the end.
 *
 * Put facets in a row marked `data-facet-row`: an unfolded facet that would
 * run past the row's right edge slides left to stay inside it, and the
 * others fade back while one is open.
 */
export function Facet<T extends string | number>({
  label,
  group,
  value,
  options,
  onChange,
  disabled = false,
  detailInValue = false,
}: {
  label: string;
  /** Unique per facet: the highlight's animations are keyed on it. */
  group: string;
  /** null when none is selected. */
  value: T | null;
  options: FacetOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  /** Show the selected option's small print next to it while folded (a limit, where the label alone is vague). */
  detailInValue?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [pointed, setPointed] = useState<T | null>(null);
  const anchor = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  const [width, setWidth] = useState<number | null>(null);
  const [shift, setShift] = useState(0);

  const selected = options.find((option) => option.id === value);
  const hasDetails = options.some((option) => option.detail);
  const caption = options.find((option) => option.id === (pointed ?? value))?.detail;

  const openNow = useCallback(() => {
    window.clearTimeout(closeTimer.current);
    if (!disabled) setOpen(true);
  }, [disabled]);
  const closeSoon = () => {
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setOpen(false), CLOSE_DELAY);
  };
  useEffect(() => () => window.clearTimeout(closeTimer.current), []);
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);
  useEffect(() => {
    if (!open) setPointed(null);
    // The row dims the other facets while any is open (it asks the DOM, so a quick
    // move from one facet to the next never leaves the row in the wrong state).
    const row = anchor.current?.closest<HTMLElement>("[data-facet-row]");
    row?.toggleAttribute("data-facet-open", row.querySelector("[aria-expanded=true]") !== null);
  }, [open]);

  // The frame's width follows its content, so it can glide between folded and unfolded.
  useLayoutEffect(() => {
    const element = content.current;
    if (!element) return;
    const measure = () => setWidth(element.offsetWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Unfolded past the row's right edge: slide left, but never past its left edge.
  useLayoutEffect(() => {
    const row = anchor.current?.closest<HTMLElement>("[data-facet-row]");
    if (!open || !row || !anchor.current || width === null) return setShift(0);
    const left = anchor.current.getBoundingClientRect().left - row.getBoundingClientRect().left;
    const next = Math.max(-left, Math.min(0, row.clientWidth - (left + width)));
    // Leaving a sliver of the facet underneath showing looks like a mistake: go flush instead.
    setShift(next < 0 && left + next < 96 ? -left : next);
  }, [open, width]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      setOpen(false);
      trigger.current?.focus();
    }
  };

  const folded = (
    <>
      <span className="text-xs text-subtle-foreground">{label}</span>
      <span className="flex items-center gap-1.5 text-[13px] font-medium whitespace-nowrap [&_svg]:size-3.5 [&_svg]:text-primary">
        {selected?.icon}
        {selected?.label ?? "—"}
        {detailInValue && selected?.detail && (
          <span className="font-normal text-muted-foreground tabular-nums">{selected.detail}</span>
        )}
      </span>
      <ChevronRight className="-ml-0.5 size-3.5 text-subtle-foreground" />
    </>
  );

  return (
    <div
      ref={anchor}
      className={cn(
        "relative h-9 transition-[opacity,filter] duration-200",
        open ? "z-20" : "z-0 in-data-facet-open:opacity-35 in-data-facet-open:blur-[0.5px]",
      )}
      onPointerEnter={(event) => event.pointerType === "mouse" && openNow()}
      onPointerLeave={(event) => event.pointerType === "mouse" && closeSoon()}
      onFocus={openNow}
      onBlur={(event) => {
        if (!anchor.current?.contains(event.relatedTarget as Node | null)) setOpen(false);
      }}
      onKeyDown={onKeyDown}
    >
      {/* Holds the folded size in the row, so the facets around it never move. */}
      <div aria-hidden className="invisible flex h-9 items-center gap-2 border pr-2 pl-3">
        {folded}
      </div>

      <motion.div
        className={cn(
          "absolute top-0 left-0 h-9 overflow-hidden rounded-xl border bg-card transition-[border-color,box-shadow] duration-200",
          open ? "border-border-strong shadow-pop" : "shadow-[0_1px_2px_color-mix(in_srgb,var(--shadow-color)_6%,transparent)]",
          disabled && "opacity-50",
        )}
        initial={false}
        animate={width === null ? { x: shift } : { width, x: shift }}
        transition={spring}
      >
        <div ref={content} className="relative flex h-full w-max items-center">
          <button
            ref={trigger}
            type="button"
            aria-expanded={open}
            aria-label={`${label}: ${selected?.text ?? (typeof selected?.label === "string" ? selected.label : "")}`}
            disabled={disabled}
            // Opens for a tap or Enter; it closes when the pointer or focus leaves, or on Escape.
            onClick={openNow}
            className="flex h-full items-center gap-2 pl-3 outline-none focus-visible:text-primary"
          >
            {open ? (
              <span className="text-xs text-subtle-foreground">{label}</span>
            ) : (
              <span className="flex items-center gap-2 pr-2">{folded}</span>
            )}
          </button>

          <AnimatePresence initial={false}>
            {open && (
              <motion.div
                key="options"
                className="flex h-full items-center"
                exit={{ opacity: 0, transition: { duration: 0.08 } }}
              >
                <span aria-hidden className="mx-2 h-4 w-px bg-border-strong" />
                <div
                  role="radiogroup"
                  aria-label={label}
                  onKeyDown={onRadioKeys}
                  onPointerLeave={() => setPointed(null)}
                  className="flex items-center gap-0.5 pr-1"
                >
                  {options.map((option, index) => {
                    const isSelected = option.id === value;
                    return (
                      <motion.button
                        key={option.id}
                        type="button"
                        role="radio"
                        aria-checked={isSelected}
                        tabIndex={isSelected || (value === null && index === 0) ? 0 : -1}
                        onClick={() => onChange(option.id)}
                        onPointerEnter={() => setPointed(option.id)}
                        onFocus={() => setPointed(option.id)}
                        initial={{ opacity: 0, x: -8, filter: "blur(4px)" }}
                        animate={{ opacity: 1, x: 0, filter: "blur(0px)", transition: { ...spring, delay: index * 0.028 } }}
                        className={cn(
                          "relative flex h-7 items-center rounded-lg px-2.5 text-[13px] font-medium whitespace-nowrap outline-none transition-colors duration-150 [&_svg]:size-3.5",
                          isSelected ? "text-primary" : "text-muted-foreground hover:text-foreground",
                        )}
                      >
                        {pointed === option.id && (
                          <motion.span
                            layoutId={`facet-hover-${group}`}
                            transition={spring}
                            className="absolute inset-0 rounded-lg bg-accent"
                          />
                        )}
                        {isSelected && (
                          <motion.span
                            layoutId={`facet-mark-${group}`}
                            transition={spring}
                            className="absolute inset-x-2.5 bottom-0.5 h-0.5 rounded-full bg-primary shadow-[0_0_8px_var(--primary)]"
                          />
                        )}
                        <span className="relative flex items-center gap-1.5">
                          {option.icon}
                          {option.label}
                        </span>
                      </motion.button>
                    );
                  })}
                </div>

                {/* The pointed-at option's small print; a fixed width keeps the frame still while it changes. */}
                {hasDetails && (
                  <span className="relative mr-3 ml-1 grid h-full w-24 items-center overflow-hidden border-l pl-3 text-xs text-muted-foreground tabular-nums">
                    <AnimatePresence initial={false} mode="popLayout">
                      <motion.span
                        key={caption ?? ""}
                        className="truncate"
                        initial={{ opacity: 0, y: 8 }}
                        animate={{ opacity: 1, y: 0 }}
                        exit={{ opacity: 0, y: -8 }}
                        transition={spring}
                      >
                        {caption ?? ""}
                      </motion.span>
                    </AnimatePresence>
                  </span>
                )}
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </motion.div>
    </div>
  );
}
