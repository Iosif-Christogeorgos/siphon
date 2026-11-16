import { X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Dialog } from "radix-ui";
import type { ReactNode } from "react";
import { spring } from "@/lib/motion";

/**
 * A panel that slides in from the right edge, over the main screen.
 * Built on Radix Dialog for focus trapping and Escape-to-close.
 */
export function Sheet({
  open,
  onOpenChange,
  title,
  description,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** Read by screen readers; shown under the title when given. */
  description?: string;
  children: ReactNode;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <AnimatePresence>
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
                className="fixed top-11 right-0 bottom-0 z-50 flex w-[380px] max-w-[calc(100vw-3rem)] flex-col border-l bg-popover shadow-pop outline-none"
                initial={{ x: "100%" }}
                animate={{ x: 0 }}
                exit={{ x: "100%" }}
                transition={spring}
              >
                <header className="flex shrink-0 items-start justify-between gap-3 px-5 pt-4 pb-3">
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
                </header>
                <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto px-5 pb-5">{children}</div>
              </motion.aside>
            </Dialog.Content>
          </Dialog.Portal>
        )}
      </AnimatePresence>
    </Dialog.Root>
  );
}
