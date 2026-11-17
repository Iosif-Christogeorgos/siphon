import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowDownToLine, Minus, Settings, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";
import { LogoGlyph } from "@/components/Logo";
import { Hint } from "@/components/ui/tooltip";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { selectActiveCount, useDownloads } from "@/stores/downloads";
import { useUi } from "@/stores/ui";

// The window is frameless (tauri.conf.json), so the title bar draws its own controls.

export function TitleBar() {
  const appWindow = getCurrentWindow();
  const openPanel = useUi((state) => state.openPanel);
  const activeDownloads = useDownloads(selectActiveCount);
  const anyDownloads = useDownloads((state) => state.order.length > 0);
  return (
    <header
      data-tauri-drag-region
      className="flex h-11 shrink-0 items-center justify-between pl-4"
    >
      {/* pointer-events-none lets drags on the label reach the drag region. */}
      <div className="pointer-events-none flex items-center gap-2 text-[13px] font-medium tracking-tight text-muted-foreground">
        <LogoGlyph className="size-3.5 text-primary" />
        Siphon
      </div>

      <div className="flex h-full items-center">
        <div className="mr-1.5 flex items-center gap-0.5">
          <Hint label="Downloads">
            <button
              type="button"
              aria-label="Downloads"
              onClick={() => openPanel("downloads")}
              className={cn(toolButton, anyDownloads && "text-foreground")}
            >
              <ArrowDownToLine className="size-4" strokeWidth={1.75} />
              <AnimatePresence>
                {activeDownloads > 0 && (
                  <motion.span
                    key="badge"
                    initial={{ scale: 0 }}
                    animate={{ scale: 1 }}
                    exit={{ scale: 0 }}
                    transition={spring}
                    className="absolute -top-0.5 -right-0.5 grid h-3.5 min-w-3.5 place-items-center rounded-full bg-primary px-1 text-[9px] leading-none font-semibold text-primary-foreground tabular-nums"
                  >
                    {activeDownloads}
                  </motion.span>
                )}
              </AnimatePresence>
            </button>
          </Hint>
          <Hint label="Settings">
            <button type="button" aria-label="Settings" onClick={() => openPanel("settings")} className={toolButton}>
              <Settings className="size-4" strokeWidth={1.75} />
            </button>
          </Hint>
        </div>
        <div className="flex h-full">
          <WindowButton label="Minimize" onClick={() => appWindow.minimize()}>
            <Minus className="size-4" strokeWidth={1.5} />
          </WindowButton>
          <WindowButton label="Maximize" onClick={() => appWindow.toggleMaximize()}>
            <Square className="size-3" strokeWidth={1.75} />
          </WindowButton>
          <WindowButton label="Close" onClick={() => appWindow.close()} danger>
            <X className="size-4" strokeWidth={1.5} />
          </WindowButton>
        </div>
      </div>
    </header>
  );
}

const toolButton =
  "relative grid size-7 place-items-center rounded-md text-muted-foreground transition-colors duration-100 hover:bg-accent hover:text-foreground";

function WindowButton({
  label,
  onClick,
  danger,
  children,
}: {
  label: string;
  onClick: () => void;
  danger?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      tabIndex={-1}
      onClick={onClick}
      className={cn(
        "grid h-full w-11.5 place-items-center text-muted-foreground transition-colors duration-100",
        danger ? "hover:bg-[#e5484d] hover:text-white" : "hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
