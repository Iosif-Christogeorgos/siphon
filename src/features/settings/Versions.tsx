import { ArrowRight, Check, LoaderCircle, RefreshCw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useState } from "react";
import { LogoMark } from "@/components/Logo";
import { Button } from "@/components/ui/button";
import { spring } from "@/lib/motion";
import { TOOL_NAMES, TOOL_ORDER, TOOL_ROLES } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { useAppUpdate } from "@/stores/appUpdate";
import { selectAvailable, useBinaries } from "@/stores/binaries";
import { useToasts } from "@/stores/toasts";
import { Card } from "./parts";

/** How long "Up to date" stays after a check that found nothing. */
const CONFIRM_FOR = 2600;

/** Siphon and its tools, each with its version, and a check on demand. */
export function Versions() {
  const tools = useBinaries((state) => state.tools);
  const syncing = useBinaries((state) => state.syncing);
  const sync = useBinaries((state) => state.sync);
  // (A boolean, not the list: a selector must return a stable value.)
  const available = useBinaries((state) => selectAvailable(state).length > 0);
  const app = useAppUpdate();
  const [upToDate, setUpToDate] = useState(false);
  useEffect(() => {
    if (!upToDate) return;
    const timer = window.setTimeout(() => setUpToDate(false), CONFIRM_FOR);
    return () => window.clearTimeout(timer);
  }, [upToDate]);
  if (!tools) return null;

  const run = async () => {
    setUpToDate(false);
    await Promise.all([sync(available), app.check()]);
    // A tool can't be swapped while a download is using it. Say so, or the
    // button would seem to have done nothing.
    const stillWaiting = selectAvailable(useBinaries.getState()).length > 0;
    if (available && stillWaiting) {
      useToasts.getState().show({
        id: "update-deferred",
        title: "The update is waiting for your downloads",
        description: "It will be installed once they have finished. Press Update now again then, or leave it to the next check.",
      });
    } else if (!stillWaiting && !useAppUpdate.getState().available) {
      setUpToDate(true);
    }
  };

  return (
    <Card>
      {/* Siphon itself, on a band of its own colour when there is a newer one to restart into. */}
      <div className={cn("relative px-4 py-3.5 transition-colors duration-300", app.available && "bg-primary-soft")}>
        <div className="flex items-center gap-3">
          <LogoMark className="size-9 shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="text-[13px] font-semibold tracking-tight">Siphon</p>
            <p className="text-xs text-muted-foreground">{app.available ? "A new version is ready" : "This app"}</p>
          </div>
          <VersionTag current={app.current ?? "…"} next={app.available} />
        </div>
        {app.available && (
          <Button size="sm" className="mt-3 w-full" disabled={app.installing} onClick={() => void app.install()}>
            {app.installing ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
            Restart to update
          </Button>
        )}
      </div>

      {TOOL_ORDER.map((tool) => {
        const status = tools[tool];
        return (
          <div
            key={tool}
            className="relative flex items-center gap-3 px-4 py-2.5 before:absolute before:inset-x-4 before:top-0 before:h-px before:bg-border"
          >
            <div className="min-w-0 flex-1">
              <p className="text-[13px] font-medium">{TOOL_NAMES[tool]}</p>
              {/* When a newer version waits, say what it changes instead. */}
              <p className={cn("truncate text-xs", status.available ? "text-primary" : "text-muted-foreground")}>
                {status.available ? status.available.summary : TOOL_ROLES[tool]}
              </p>
            </div>
            <VersionTag current={status.version ?? "not installed"} next={status.available?.version} />
          </div>
        );
      })}

      <div className="flex items-center justify-between gap-3 border-t bg-accent/50 py-2 pr-2 pl-4">
        <div className="relative h-4 min-w-0 flex-1 text-xs text-subtle-foreground">
          <AnimatePresence mode="popLayout" initial={false}>
            {upToDate ? (
              <Swap key="done">
                <span className="flex items-center gap-1.5 text-success">
                  <Check className="size-3.5" strokeWidth={2.5} />
                  Everything is up to date
                </span>
              </Swap>
            ) : (
              <Swap key="when">Checked at launch and daily</Swap>
            )}
          </AnimatePresence>
        </div>
        <Button variant="ghost" size="sm" className="h-7 px-2.5" disabled={syncing} onClick={() => void run()}>
          {syncing ? <LoaderCircle className="size-3.5 animate-spin" /> : <RefreshCw className="size-3.5" />}
          {syncing ? "Checking…" : available ? "Update now" : "Check now"}
        </Button>
      </div>
    </Card>
  );
}

/** A version, and the newer one waiting, if any. */
function VersionTag({ current, next }: { current: string; next?: string | null }) {
  return (
    <span className="flex min-w-0 shrink items-center gap-1 font-mono text-[11px] tabular-nums">
      <span
        title={current}
        className={cn(
          "max-w-28 truncate rounded-md bg-secondary px-1.5 py-0.5 text-muted-foreground",
          next && "line-through decoration-subtle-foreground/60",
        )}
      >
        {current}
      </span>
      {next && (
        <>
          <ArrowRight className="size-3 shrink-0 text-primary" />
          <span title={next} className="max-w-28 truncate rounded-md bg-primary-soft px-1.5 py-0.5 text-primary ring-1 ring-primary/20">
            {next}
          </span>
        </>
      )}
    </span>
  );
}

function Swap({ children }: { children: ReactNode }) {
  return (
    <motion.span
      className="absolute inset-0 flex items-center truncate"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={spring}
    >
      {children}
    </motion.span>
  );
}
