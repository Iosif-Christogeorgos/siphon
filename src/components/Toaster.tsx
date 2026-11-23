import { X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useRef } from "react";
import { spring } from "@/lib/motion";
import { type Toast, useToasts } from "@/stores/toasts";

/** Long enough to read two lines; hovering holds a toast open. */
const VISIBLE_MS = 8000;

/** Quiet notices at the bottom of the window. */
export function Toaster() {
  const toasts = useToasts((state) => state.toasts);
  return (
    // A toast's button can be pressed with a panel open, which stays open.
    <div data-beside-panels className="pointer-events-none fixed inset-x-0 bottom-5 z-[60] flex flex-col items-center gap-2 px-6">
      <AnimatePresence initial={false}>
        {toasts.map((toast) => (
          <motion.div
            key={toast.id}
            layout
            initial={{ opacity: 0, y: 24, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.96, transition: { duration: 0.15 } }}
            transition={spring}
          >
            <ToastCard toast={toast} />
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}

function ToastCard({ toast }: { toast: Toast }) {
  const dismiss = useToasts((state) => state.dismiss);
  const timer = useRef<number | undefined>(undefined);

  const start = useCallback(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => dismiss(toast.id), VISIBLE_MS);
  }, [dismiss, toast.id]);
  const hold = useCallback(() => window.clearTimeout(timer.current), []);
  // A replaced toast (same id, new text) gets its full time again.
  useEffect(() => {
    start();
    return hold;
  }, [start, hold, toast]);

  return (
    <div
      role="status"
      onMouseEnter={hold}
      onMouseLeave={start}
      className="pointer-events-auto flex max-w-[30rem] items-center gap-3 rounded-2xl border bg-popover py-2.5 pr-2 pl-4 shadow-pop"
    >
      <span className="size-1.5 shrink-0 rounded-full bg-primary" />
      <div className="min-w-0">
        <p className="truncate text-[13px] font-medium">{toast.title}</p>
        {toast.description && (
          <p className="line-clamp-2 text-xs text-pretty text-muted-foreground">{toast.description}</p>
        )}
      </div>
      {toast.action && (
        <button
          type="button"
          onClick={() => {
            toast.action?.run();
            dismiss(toast.id);
          }}
          className="shrink-0 rounded-lg bg-primary-soft px-2.5 py-1.5 text-xs font-medium text-primary transition-colors hover:bg-primary/20"
        >
          {toast.action.label}
        </button>
      )}
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => dismiss(toast.id)}
        className="grid size-7 shrink-0 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}
