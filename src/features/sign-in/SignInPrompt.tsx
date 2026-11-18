import { Info, LoaderCircle, LogIn, TriangleAlert } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Facet } from "@/components/ui/facet";
import { Hint } from "@/components/ui/tooltip";
import { spring } from "@/lib/motion";
import { type MethodId, methodIds, methodOf, suggestedMethod, useSignIn } from "@/stores/signIn";
import type { SignInNeed } from "@/types";

/** What a refused attempt said: a line, and the reason behind the ⓘ. */
interface Refusal {
  short: string;
  reason: string;
}

/** The ways to sign in, explained once, behind every prompt's ⓘ. */
const METHODS =
  "Sign-in window: the site’s own page, inside Siphon. A browser: takes over your sign-in from it (Firefox works best). cookies.txt: a file of cookies exported from your browser with an extension.";

/** The prompt's line, what's behind its ⓘ, and its button, for each situation. */
function wording(need: SignInNeed, message: string): { title: ReactNode; why: string; action: string } {
  const site = <span className="text-primary">{need.site}</span>;
  if (need.chance === "needed" && need.refused) {
    return {
      title: <>Your {site} sign-in has run out</>,
      why: `${need.site} no longer accepts the sign-in Siphon kept. Sign in again to carry on.`,
      action: "Sign in again",
    };
  }
  switch (need.chance) {
    case "needed":
      return {
        title: <>{site} needs you to sign in</>,
        why: "This video is only for signed-in viewers. Signing in with your own account unlocks what that account can watch, nothing more.",
        action: "Sign in",
      };
    case "maybe":
      return {
        title: <>This might work with your {site} account</>,
        why: `${message} Signing in unlocks it only if your account has access: a membership, a paid tier, or a video shared with you.`,
        action: "Sign in",
      };
    case "risky":
      return {
        title: <>{site} wants to check you’re not a bot</>,
        why: "Signing in usually gets past this check, but downloading a lot with an account can get that account flagged. The check also passes on its own within a few hours.",
        action: "Sign in anyway",
      };
    case "unknown":
      return {
        title: <>Signing in to {site} might help</>,
        why: `${need.site} didn’t say why this failed (${message}). An account sometimes gets past that, but often won’t.`,
        action: "Sign in",
      };
  }
}

/**
 * Where an error says signing in could help: the site and how likely it is
 * to help, how to sign in (Siphon's window, a browser here, or a cookies.txt
 * file: one facet, the suggested way chosen), Sign in, and No thanks. A way
 * that doesn't work says why in a line, and the choice moves on to the next.
 */
export function SignInPrompt({
  need,
  message,
  url,
  onSignedIn,
  onDecline,
}: {
  need: SignInNeed;
  /** The error's own explanation, for the ⓘ. */
  message: string;
  /** The video that needed it: the sign-in is checked on it. */
  url: string;
  onSignedIn?: () => void;
  /** No thanks: back to the plain error. */
  onDecline?: () => void;
}) {
  const { site } = need;
  const browsers = useSignIn((state) => state.browsers);
  const failed = useSignIn((state) => state.failed[site]);
  const busy = useSignIn((state) => state.busy);
  const [choice, setChoice] = useState<MethodId | null>(null);
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const { title, why, action } = wording(need, message);

  useEffect(() => void useSignIn.getState().loadBrowsers(), []);
  // The suggestion, until the user picks.
  const suggested = browsers ? suggestedMethod(browsers, failed ?? []) : null;
  useEffect(() => {
    if (suggested && choice === null) setChoice(suggested);
  }, [suggested, choice]);

  const working = busy === site;
  const run = async () => {
    if (!choice) return;
    setRefusal(null);
    const outcome = await useSignIn.getState().run(url, site, methodOf(choice));
    if (outcome?.outcome === "refused") {
      setRefusal({ short: outcome.short, reason: outcome.reason });
      // On to the next way worth trying; the user can still pick this one again (a corrected file, say).
      setChoice(suggestedMethod(browsers ?? [], useSignIn.getState().failed[site] ?? []));
    }
    if (outcome?.outcome === "signed_in") onSignedIn?.();
  };

  const options = methodIds(browsers ?? []).map((id) => ({
    id,
    label:
      id === "window"
        ? "Sign-in window"
        : id === "file"
          ? "cookies.txt"
          : (browsers?.find((b) => `browser:${b.id}` === id)?.name ?? id),
  }));
  // Buttons fade under the facet as it unfolds, like neighbouring facets.
  const underFacet = "transition-opacity duration-150 in-data-facet-open:opacity-0";

  return (
    <div className="min-w-0 flex-1">
      <div className="flex min-w-0 items-center gap-1">
        <h2 className="truncate font-semibold tracking-tight">{title}</h2>
        <Hint
          label={
            <span className="block space-y-1.5">
              <span className="block">{why}</span>
              <span className="block text-muted-foreground">{METHODS}</span>
            </span>
          }
          className="max-w-80 px-3 py-2 leading-relaxed text-pretty"
        >
          <button
            type="button"
            aria-label="About signing in"
            className="grid size-6 shrink-0 place-items-center rounded-full text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <Info className="size-3.5" />
          </button>
        </Hint>
      </div>

      <div data-facet-row className="relative mt-2.5 flex items-center gap-2">
        <Facet
          label="Sign in with"
          group={`sign-in-${site}`}
          value={choice}
          options={options}
          onChange={setChoice}
          disabled={working || !browsers}
        />
        {onDecline && (
          <Button
            variant="ghost"
            size="sm"
            className={`ml-auto ${underFacet}`}
            disabled={working}
            onClick={onDecline}
          >
            No thanks
          </Button>
        )}
        <Button
          size="sm"
          variant={need.chance === "risky" ? "secondary" : "default"}
          className={onDecline ? underFacet : `ml-auto ${underFacet}`}
          disabled={working || !choice}
          onClick={() => void run()}
        >
          {working ? <LoaderCircle className="animate-spin" /> : <LogIn />}
          {working ? "Signing in…" : action}
        </Button>
      </div>

      <AnimatePresence initial={false}>
        {refusal && !working && (
          <motion.div
            key={refusal.short}
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={spring}
            className="overflow-hidden"
          >
            <p role="alert" className="mt-2 flex items-center gap-1.5 text-[13px]">
              <TriangleAlert className="size-3.5 shrink-0 text-destructive" />
              <span className="truncate">{refusal.short}</span>
              <Hint label={refusal.reason} className="max-w-72 px-3 py-2 leading-relaxed text-pretty">
                <button
                  type="button"
                  aria-label="Why"
                  className="grid size-6 shrink-0 place-items-center rounded-full text-primary transition-colors hover:bg-accent"
                >
                  <Info className="size-3.5" />
                </button>
              </Hint>
            </p>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
