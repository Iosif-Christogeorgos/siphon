import { getCurrentWindow } from "@tauri-apps/api/window";
import { Pause } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { AlertDialog } from "radix-ui";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { spring } from "@/lib/motion";
import { saveDownloadsNow, selectActiveCount, useDownloads } from "@/stores/downloads";

/**
 * Asks before the window closes while downloads run or wait (title-bar
 * button, Alt+F4 or the taskbar alike). Closing pauses them, and they carry
 * on next time, so this is a heads-up rather than a warning.
 */
export function CloseGuard() {
  const [asking, setAsking] = useState(false);
  const active = useDownloads(selectActiveCount);
  const confirmed = useRef(false);

  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested((event) => {
      if (!confirmed.current && selectActiveCount(useDownloads.getState()) > 0) {
        event.preventDefault();
        setAsking(true);
        return;
      }
      saveDownloadsNow();
    });
    return () => void unlisten.then((stop) => stop());
  }, []);

  const close = () => {
    confirmed.current = true;
    saveDownloadsNow();
    void getCurrentWindow().destroy();
  };

  const running = useDownloads((state) => state.order.some((id) => state.byId[id]?.status === "running"));
  const what = `${active} ${active === 1 ? "download is" : "downloads are"} still ${running ? "running" : "waiting"}`;

  return (
    <AlertDialog.Root open={asking} onOpenChange={setAsking}>
      <AnimatePresence>
        {asking && (
          <AlertDialog.Portal forceMount>
            <AlertDialog.Overlay asChild>
              <motion.div
                className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.15 }}
              />
            </AlertDialog.Overlay>
            <AlertDialog.Content asChild>
              <motion.div
                className="fixed top-1/2 left-1/2 z-50 w-[min(380px,calc(100vw-2rem))] rounded-2xl border bg-popover p-5 shadow-pop outline-none"
                initial={{ opacity: 0, scale: 0.96, x: "-50%", y: "-48%" }}
                animate={{ opacity: 1, scale: 1, x: "-50%", y: "-50%" }}
                exit={{ opacity: 0, scale: 0.96, x: "-50%", y: "-48%", transition: { duration: 0.12 } }}
                transition={spring}
              >
                <div className="flex gap-3.5">
                  <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary-soft text-primary">
                    <Pause className="size-4 fill-current" />
                  </div>
                  <div className="min-w-0">
                    <AlertDialog.Title className="text-[15px] font-semibold tracking-tight">Close Siphon?</AlertDialog.Title>
                    <AlertDialog.Description className="mt-1 text-[13px] text-pretty text-muted-foreground">
                      {what}. {active === 1 ? "It’ll" : "They’ll"} pause and carry on where{" "}
                      {active === 1 ? "it" : "they"} stopped when you open Siphon again.
                    </AlertDialog.Description>
                  </div>
                </div>
                <div className="mt-5 flex justify-end gap-2">
                  <AlertDialog.Cancel asChild>
                    <Button variant="secondary" size="sm" autoFocus>
                      Keep downloading
                    </Button>
                  </AlertDialog.Cancel>
                  <AlertDialog.Action asChild>
                    <Button size="sm" onClick={close}>
                      Close and pause
                    </Button>
                  </AlertDialog.Action>
                </div>
              </motion.div>
            </AlertDialog.Content>
          </AlertDialog.Portal>
        )}
      </AnimatePresence>
    </AlertDialog.Root>
  );
}
