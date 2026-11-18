import { Info, LoaderCircle, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { LogoGlyph } from "@/components/Logo";
import { Button } from "@/components/ui/button";
import { Hint } from "@/components/ui/tooltip";
import { ipc, onSignInStatus } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { SignInStatus } from "@/types";

/**
 * The bar across the top of the sign-in window, above the site's own page:
 * which site, what to do, and Done. The window has no frame of its own, so
 * the bar is also where it is dragged and closed.
 */
export function SignInBar() {
  const [site, setSite] = useState<string | null>(null);
  const [state, setState] = useState<SignInStatus>({ phase: "waiting" });
  const status = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void ipc.signInSite().then(setSite);
    const unlisten = onSignInStatus(setState);
    // Escape closes the window, as it would a dialog.
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && void ipc.signInCancel();
    window.addEventListener("keydown", onKey);
    return () => {
      void unlisten.then((stop) => stop());
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  const checking = state.phase === "checking";
  const line =
    state.phase === "checking"
      ? "Checking your sign-in…"
      : state.phase === "not_yet"
        ? "Not signed in yet. Finish on the page, then press Done."
        : state.phase === "failed"
          ? state.message
          : "Sign in on the page below, then press Done.";

  return (
    <header data-tauri-drag-region className="flex h-[52px] items-center gap-3 border-b bg-background pl-4">
      {/* pointer-events-none lets drags on the text reach the drag region. */}
      <LogoGlyph className="pointer-events-none size-4 shrink-0 text-primary" />
      <div className="pointer-events-none min-w-0 flex-1">
        <p className="truncate text-[13px] font-medium tracking-tight">
          Sign in to <span className="text-primary">{site ?? "…"}</span>
        </p>
        {/* The whole line on hover, only when it didn't fit (an error can be long). */}
        <Hint
          label={line}
          side="bottom"
          className="max-w-80 text-pretty"
          when={() => {
            const shown = status.current?.querySelector("p");
            return !!shown && shown.scrollWidth > shown.clientWidth;
          }}
        >
          <div ref={status} className="pointer-events-auto relative h-4 overflow-hidden">
            <AnimatePresence initial={false}>
              <motion.p
                key={line}
                role="status"
                className={cn(
                  "absolute inset-0 truncate text-xs leading-4",
                  state.phase === "not_yet" || state.phase === "failed" ? "text-destructive" : "text-muted-foreground",
                )}
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -8 }}
                transition={spring}
              >
                {line}
              </motion.p>
            </AnimatePresence>
          </div>
        </Hint>
      </div>

      <Hint
        label={`Siphon keeps only ${site ?? "this site"}’s cookies from this window, encrypted for your Windows account. It never sees your password. Sign out any time in Settings.`}
        className="max-w-72 px-3 py-2 leading-relaxed text-pretty"
        side="bottom"
      >
        <button
          type="button"
          aria-label="About signing in"
          className="grid size-7 shrink-0 place-items-center rounded-full text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <Info className="size-3.5" />
        </button>
      </Hint>
      <Button size="sm" disabled={checking} onClick={() => void ipc.signInDone()} className="min-w-20">
        {checking && <LoaderCircle className="size-3.5 animate-spin" />}
        Done
      </Button>
      <Hint label="Cancel" side="bottom">
        <button
          type="button"
          aria-label="Cancel signing in"
          onClick={() => void ipc.signInCancel()}
          className="grid h-full w-11.5 shrink-0 place-items-center text-muted-foreground transition-colors duration-100 hover:bg-[#e5484d] hover:text-white"
        >
          <X className="size-4" strokeWidth={1.5} />
        </button>
      </Hint>
    </header>
  );
}
