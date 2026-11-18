import {
  ChevronRight,
  CircleAlert,
  HardDrive,
  Lock,
  LogIn,
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
import { Hint } from "@/components/ui/tooltip";
import { SignInPrompt } from "@/features/sign-in/SignInPrompt";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { AppError, ErrorKind, SignInNeed } from "@/types";

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

/** What the card does about an account, for an error that says one could help. */
type AccountView =
  /** The sign-in prompt, open. */
  | "prompt"
  /** The plain error, with a quiet way to the prompt. */
  | "quiet"
  /** Signed in already: the plain error, with "Try with your sign-in". */
  | "use"
  /** Signed in, and refused all the same: this account has no access. */
  | "no_access";

/**
 * Siphon always tries without an account first, so an offer only ever
 * follows a refusal. It opens by itself only where an account is the likely
 * fix: a login wall (or a sign-in that ran out), or members-only and the
 * like. The bot check and errors Siphon doesn't recognise only get a quiet
 * link, and so does any offer the user turned down.
 */
function accountView(need: SignInNeed, declined: boolean, opened: boolean): AccountView | null {
  if (need.refused) {
    if (need.chance === "needed") return "prompt";
    return need.chance === "maybe" ? "no_access" : null;
  }
  if (need.signed_in) return "use";
  const likely = need.chance === "needed" || need.chance === "maybe";
  return opened || (likely && !declined) ? "prompt" : "quiet";
}

/**
 * A friendly error with the raw log tucked behind a "Details" toggle. Where
 * signing in to the site could help, and the card knows the video
 * (`signIn`), it says so, as plainly as the odds allow (see `accountView`).
 */
export function ErrorCard({
  error,
  action,
  signIn,
}: {
  error: AppError;
  action?: ReactNode;
  signIn?: {
    url: string;
    onSignedIn?: () => void;
    /** Run this again with the site's sign-in Siphon already has. */
    onUseAccount?: () => void;
  };
}) {
  const [open, setOpen] = useState(false);
  const [declined, setDeclined] = useState(false);
  const [opened, setOpened] = useState(false);
  const { icon: Icon, title: plainTitle } = PRESENTATION[error.kind];
  const need = signIn ? error.sign_in : null;
  const view = need ? accountView(need, declined, opened) : null;
  const title = view === "no_access" && need ? `Your ${need.site} account doesn’t have access` : plainTitle;

  return (
    <div role="alert" className="rounded-[1.25rem] border bg-card p-4 shadow-card">
      <div className="flex gap-3.5">
        <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-destructive-soft text-destructive">
          <Icon className="size-4.5" />
        </div>
        {view === "prompt" && need && signIn ? (
          <SignInPrompt
            need={need}
            message={error.message}
            url={signIn.url}
            onSignedIn={signIn.onSignedIn}
            onDecline={() => {
              setDeclined(true);
              setOpened(false);
            }}
          />
        ) : (
          <>
            <div className="min-w-0 flex-1">
              <h2 className="font-semibold tracking-tight">{title}</h2>
              <p data-selectable className="mt-0.5 text-[13px] text-pretty text-muted-foreground">
                {error.message}
              </p>
              {need && (view === "quiet" || view === "use") && (
                <AccountLink
                  need={need}
                  label={
                    view === "use"
                      ? `Try with your ${need.site} sign-in`
                      : need.chance === "risky"
                        ? "Sign in anyway…"
                        : "Sign in…"
                  }
                  onClick={view === "use" ? signIn?.onUseAccount : () => setOpened(true)}
                />
              )}
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

/** A quiet way to an account: for the odds that are low, risky, or turned down once. */
function AccountLink({ need, label, onClick }: { need: SignInNeed; label: string; onClick?: () => void }) {
  if (!onClick) return null;
  const link = (
    <button
      type="button"
      onClick={onClick}
      className="mt-1.5 flex items-center gap-1 rounded-md text-xs font-medium text-subtle-foreground transition-colors hover:text-primary focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none"
    >
      <LogIn className="size-3.5" />
      {label}
    </button>
  );
  // The risk is said before anyone takes it.
  return need.chance === "risky" ? (
    <Hint
      label="Signing in usually gets past this check, but downloading a lot with an account can get that account flagged. A second account is safer."
      className="max-w-72 px-3 py-2 leading-relaxed text-pretty"
    >
      {link}
    </Hint>
  ) : (
    link
  );
}
