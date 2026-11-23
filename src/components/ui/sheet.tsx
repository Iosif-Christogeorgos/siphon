import { X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Dialog } from "radix-ui";
import { type ReactNode, type Ref, useState } from "react";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";

/**
 * A panel that slides in from the right edge, over the main screen.
 * Built on Radix Dialog for focus trapping and Escape-to-close.
 */
export function Sheet({
  open,
  onOpenChange,
  title,
  description,
  toolbar,
  bodyRef,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** Read by screen readers; shown under the title when given. */
  description?: string;
  /** Stays in view under the title while the body scrolls (a panel's own tabs). */
  toolbar?: ReactNode;
  /** The scrolling body, for a panel that follows or moves its scroll position. */
  bodyRef?: Ref<HTMLDivElement>;
  children: ReactNode;
}) {
  // A soft shade under the header once something has scrolled beneath it.
  const [scrolled, setScrolled] = useState(false);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <AnimatePresence onExitComplete={() => setScrolled(false)}>
        {open && (
          <Dialog.Portal forceMount>
            <Dialog.Overlay asChild>
              <motion.div
                className="fixed inset-0 top-11 z-40 bg-black/30 backdrop-blur-[1.5px]"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.18 }}
              />
            </Dialog.Overlay>
            <Dialog.Content
              asChild
              {...(description ? {} : { "aria-describedby": undefined })}
              // Focus the panel itself, not its close button: no focus ring flashes on open.
              onOpenAutoFocus={(event) => {
                event.preventDefault();
                (event.currentTarget as HTMLElement).focus();
              }}
            >
              <motion.aside
                className="fixed top-11 right-0 bottom-0 z-50 flex w-[380px] max-w-[calc(100vw-3rem)] flex-col overflow-hidden border-l bg-sheet shadow-pop outline-none"
                initial={{ x: "100%" }}
                animate={{ x: 0 }}
                exit={{ x: "100%" }}
                transition={spring}
              >
                {/* The same faint wash of aqua as behind the top of the window, so the panel belongs to it. */}
                <div className="pointer-events-none absolute inset-x-0 top-0 h-56 bg-[radial-gradient(90%_100%_at_70%_0%,var(--primary-soft),transparent)]" />
                <header
                  className={cn(
                    "relative z-10 shrink-0",
                    "after:pointer-events-none after:absolute after:inset-x-0 after:top-full after:h-3 after:bg-linear-to-b after:from-(--shadow-color)/12 after:to-transparent after:opacity-0 after:transition-opacity after:duration-200",
                    scrolled && "after:opacity-100",
                  )}
                >
                  <div className="flex items-start justify-between gap-3 px-5 pt-4 pb-3">
                    <div className="min-w-0">
                      <Dialog.Title className="text-[15px] font-semibold tracking-tight">{title}</Dialog.Title>
                      {description && (
                        <Dialog.Description className="mt-0.5 text-[13px] text-muted-foreground">
                          {description}
                        </Dialog.Description>
                      )}
                    </div>
                    <Dialog.Close
                      aria-label="Close"
                      className="-mr-1.5 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    >
                      <X className="size-4" />
                    </Dialog.Close>
                  </div>
                  {toolbar}
                </header>
                <div
                  ref={bodyRef}
                  onScroll={(event) => setScrolled(event.currentTarget.scrollTop > 2)}
                  className="scrollbar-thin relative min-h-0 flex-1 overflow-y-auto px-5 pb-5"
                >
                  {children}
                </div>
              </motion.aside>
            </Dialog.Content>
          </Dialog.Portal>
        )}
      </AnimatePresence>
    </Dialog.Root>
  );
}
