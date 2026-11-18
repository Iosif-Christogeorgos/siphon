import {
  ChevronRight,
  CircleAlert,
  HardDrive,
  Lock,
  type LucideIcon,
  RefreshCw,
  Scissors,
  ShieldAlert,
  Unlink,
  VideoOff,
  WifiOff,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useState } from "react";
import { SignInPrompt } from "@/features/sign-in/SignInPrompt";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { AppError, ErrorKind } from "@/types";

const PRESENTATION: Record<ErrorKind, { icon: LucideIcon; title: string }> = {
  drm: { icon: ShieldAlert, title: "This video is copy-protected" },
  unsupported: { icon: Unlink, title: "Link not recognised" },
  unavailable: { icon: VideoOff, title: "Video unavailable" },
  login_required: { icon: Lock, title: "Sign-in required" },
  outdated: { icon: RefreshCw, title: "The site pushed back" },
  network: { icon: WifiOff, title: "Connection problem" },
  disk_full: { icon: HardDrive, title: "Not enough disk space" },
  setup: { icon: CircleAlert, title: "Not ready yet" },
  busy: { icon: CircleAlert, title: "Busy right now" },
  too_long: { icon: Scissors, title: "Too long for this limit" },
  cancelled: { icon: CircleAlert, title: "Cancelled" },
  internal: { icon: CircleAlert, title: "Something went wrong" },
};

/**
 * A friendly error with the raw log tucked behind a "Details" toggle. Where
 * signing in to the site can fix it, and the card knows the video (`signIn`),
 * it offers that instead of the plain message.
 */
export function ErrorCard({
  error,
  action,
  signIn,
}: {
  error: AppError;
  action?: ReactNode;
  signIn?: { url: string; onSignedIn?: () => void };
}) {
  const [open, setOpen] = useState(false);
  const { icon: Icon, title } = PRESENTATION[error.kind];
  const offer = error.sign_in && signIn ? { need: error.sign_in, ...signIn } : null;

  return (
    <div role="alert" className="rounded-[1.25rem] border bg-card p-4 shadow-card">
      <div className="flex gap-3.5">
        <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-destructive-soft text-destructive">
          <Icon className="size-4.5" />
        </div>
        {offer ? (
          <SignInPrompt need={offer.need} url={offer.url} onSignedIn={offer.onSignedIn} />
        ) : (
          <>
            <div className="min-w-0 flex-1">
              <h2 className="font-semibold tracking-tight">{title}</h2>
              <p data-selectable className="mt-0.5 text-[13px] text-pretty text-muted-foreground">
                {error.message}
              </p>
            </div>
            {action && <div className="shrink-0">{action}</div>}
          </>
        )}
      </div>

      {error.details && (
        <div className="mt-3 pl-12.5">
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className="flex items-center gap-1 rounded-md text-xs font-medium text-subtle-foreground transition-colors hover:text-foreground"
          >
            <ChevronRight
              className={cn("size-3.5 transition-transform duration-200", open && "rotate-90")}
            />
            Details
          </button>
          <AnimatePresence initial={false}>
            {open && (
              <motion.div
                initial={{ height: 0, opacity: 0 }}
                animate={{ height: "auto", opacity: 1 }}
                exit={{ height: 0, opacity: 0 }}
                transition={spring}
                className="overflow-hidden"
              >
                <pre
                  data-selectable
                  className="scrollbar-thin mt-2 max-h-40 overflow-auto rounded-lg bg-secondary p-3 font-mono text-[11px] leading-relaxed whitespace-pre-wrap text-muted-foreground"
                >
                  {error.details}
                </pre>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      )}
    </div>
  );
}
