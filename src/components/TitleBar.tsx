import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";
import type { ReactNode } from "react";
import { LogoGlyph } from "@/components/Logo";
import { cn } from "@/lib/utils";

// macOS keeps its native traffic lights (see tauri.macos.conf.json);
// everywhere else the window is frameless and we draw the controls.
const isMac = navigator.userAgent.includes("Mac");

export function TitleBar() {
  const appWindow = getCurrentWindow();
  return (
    <header
      data-tauri-drag-region
      className={cn("flex h-11 shrink-0 items-center justify-between", isMac ? "pl-20" : "pl-4")}
    >
      {/* pointer-events-none lets drags on the label reach the drag region. */}
      <div className="pointer-events-none flex items-center gap-2 text-[13px] font-medium tracking-tight text-muted-foreground">
        <LogoGlyph className="size-3.5 text-primary" />
        Siphon
      </div>

      {!isMac && (
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
      )}
    </header>
  );
}

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
