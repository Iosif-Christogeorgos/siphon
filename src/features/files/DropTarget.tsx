import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FilePlay } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { spring } from "@/lib/motion";

/**
 * While files are dragged over the window: a target over all of it, with the
 * focused link field's aqua edge and glow, saying what dropping them does.
 * The drop itself is taken by the backend, from the window's own event, which
 * carries the files' paths (see `local` in the backend): the window never
 * names a file. What it took in arrives as `files://dropped`.
 */
export function DropTarget() {
  const [over, setOver] = useState(false);

  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      // Only files: a link dragged from a browser brings no paths.
      if (event.payload.type === "enter") setOver(event.payload.paths.length > 0);
      else if (event.payload.type !== "over") setOver(false);
    });
    return () => void unlisten.then((stop) => stop());
  }, []);

  return (
    <AnimatePresence>
      {over && (
        <motion.div
          key="drop"
          aria-hidden
          // Below the title bar, over everything else, the panels included.
          className="pointer-events-none fixed inset-x-0 top-11 bottom-0 z-[70] p-3"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0, transition: { duration: 0.16 } }}
        >
          <div className="absolute inset-0 bg-background/55 backdrop-blur-[3px]" />
          <motion.div
            className="relative grid size-full place-items-center rounded-[1.75rem] border-2 border-dashed border-primary/75 bg-primary-soft shadow-[0_0_0_1px_color-mix(in_oklab,var(--primary)_22%,transparent),0_0_48px_-6px_var(--primary-glow)]"
            initial={{ scale: 0.97 }}
            animate={{ scale: 1 }}
            exit={{ scale: 0.98 }}
            transition={spring}
          >
            <motion.div
              className="flex flex-col items-center text-center"
              initial={{ opacity: 0, y: 8, filter: "blur(4px)" }}
              animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              transition={{ ...spring, delay: 0.04 }}
            >
              <div className="grid size-14 place-items-center rounded-2xl bg-card text-primary shadow-card">
                <FilePlay className="size-6" />
              </div>
              <p className="mt-4 text-[17px] font-semibold tracking-tight">Drop to convert</p>
              <p className="mt-1 text-[13px] text-muted-foreground">Videos, sounds, or a folder of them</p>
            </motion.div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
