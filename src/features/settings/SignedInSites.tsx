import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Info, KeyRound, LogOut } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect } from "react";
import { Segmented } from "@/components/ui/segmented";
import { Hint } from "@/components/ui/tooltip";
import { formatDay } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useSignIn } from "@/stores/signIn";
import type { SignInMode, SiteSession } from "@/types";
import { Card } from "./parts";

/**
 * The sites Siphon is signed in to: each with whether its sign-in still works
 * and when it was last used, how it was made behind the ⓘ, when to use it,
 * and Sign out.
 */
export function SignedInSites() {
  const sites = useQuery({ queryKey: ["signed-in-sites"], queryFn: ipc.signedInSites, staleTime: 0 });
  const queryClient = useQueryClient();
  const browsers = useSignIn((state) => state.browsers);
  useEffect(() => void useSignIn.getState().loadBrowsers(), []);
  const signOut = async (site: string) => {
    await ipc.signOut(site);
    await queryClient.invalidateQueries({ queryKey: ["signed-in-sites"] });
  };
  const setMode = async (site: string, mode: SignInMode) => {
    // Shown at once; the list is read again once it is stored.
    queryClient.setQueryData<SiteSession[]>(["signed-in-sites"], (list) =>
      list?.map((session) => (session.site === site ? { ...session, mode } : session)),
    );
    await ipc.setSignInMode(site, mode).finally(() => queryClient.invalidateQueries({ queryKey: ["signed-in-sites"] }));
  };
  const how = ({ method }: SiteSession) =>
    method.type === "window"
      ? "Siphon’s sign-in window"
      : method.type === "file"
        ? "a cookies.txt file"
        : (browsers?.find((browser) => browser.id === method.browser)?.name ?? method.browser);

  if (!sites.data?.length) {
    return (
      <Card className="flex items-center gap-3.5 px-4 py-4">
        <div className="grid size-9 shrink-0 place-items-center rounded-full border border-dashed border-border-strong text-subtle-foreground">
          <KeyRound className="size-4" strokeWidth={1.75} />
        </div>
        <p className="text-xs text-pretty text-muted-foreground">
          {sites.isPending ? "…" : "None yet. When a video needs an account, Siphon offers to sign in."}
        </p>
      </Card>
    );
  }

  return (
    <Card>
      <AnimatePresence initial={false}>
        {sites.data.map((session, index) => (
          <motion.div
            key={session.site}
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={spring}
            className="overflow-hidden"
          >
            <div
              className={cn(
                "relative px-4 py-3.5",
                index > 0 && "before:absolute before:inset-x-4 before:top-0 before:h-px before:bg-border",
              )}
            >
              <div className="flex items-center gap-3">
                <Monogram site={session.site} />
                <div className="min-w-0 flex-1">
                  <div className="flex min-w-0 items-center gap-1">
                    <p className="truncate text-[13px] font-medium">{session.site}</p>
                    <Hint
                      label={`Signed in with ${how(session)}, ${formatDay(session.signed_in_at * 1000)}. Siphon keeps only ${session.site}’s cookies, encrypted for your Windows account, and gives them to that site’s downloads alone.`}
                      className="max-w-72 px-3 py-2 leading-relaxed text-pretty"
                    >
                      <button
                        type="button"
                        aria-label={`About the ${session.site} sign-in`}
                        className="grid size-5 shrink-0 place-items-center rounded-full text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
                      >
                        <Info className="size-3" />
                      </button>
                    </Hint>
                  </div>
                  <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <span
                      aria-hidden
                      className={cn(
                        "size-1.5 shrink-0 rounded-full",
                        session.expired ? "bg-destructive" : "bg-success shadow-[0_0_6px_var(--success)]",
                      )}
                    />
                    {session.expired ? (
                      <span className="text-destructive">Expired · sign in again when a video asks</span>
                    ) : (
                      <span className="truncate">
                        <span className="text-foreground/90">Active</span>
                        {session.last_used && ` · used ${formatDay(session.last_used * 1000)}`}
                      </span>
                    )}
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => void signOut(session.site)}
                  className="-mr-1 flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-[13px] font-medium text-muted-foreground transition-colors duration-150 hover:bg-destructive-soft hover:text-destructive [&_svg]:size-3.5"
                >
                  <LogOut />
                  Sign out
                </button>
              </div>
              <div className="mt-3 flex items-center justify-between gap-3 pl-12">
                <span className="text-xs text-subtle-foreground">Use it</span>
                <Segmented
                  label={`When to use the ${session.site} sign-in`}
                  group={`sign-in-mode-${session.site}`}
                  value={session.mode}
                  options={[
                    {
                      id: "when_needed",
                      label: "When needed",
                      hint: "Only for videos that need it. Everything else downloads without your account.",
                    },
                    {
                      id: "always",
                      label: "Always",
                      hint: "For every video from this site. On some sites an account gets better quality.",
                    },
                  ]}
                  onChange={(mode) => void setMode(session.site, mode)}
                />
              </div>
            </div>
          </motion.div>
        ))}
      </AnimatePresence>
    </Card>
  );
}

/** A site's first letter on a tile, in place of its icon (which would mean asking the site for it). */
function Monogram({ site }: { site: string }) {
  const letter = site.replace(/^www\./, "").charAt(0).toUpperCase();
  return (
    <div className="grid size-9 shrink-0 place-items-center rounded-xl bg-linear-to-br from-primary/25 to-primary/5 text-[15px] font-semibold text-primary ring-1 ring-primary/15 ring-inset">
      {/\d/.test(letter) ? <KeyRound className="size-4" strokeWidth={1.75} /> : letter}
    </div>
  );
}
