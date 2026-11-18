import { create } from "zustand";
import { ipc, toAppError } from "@/lib/ipc";
import { queryClient } from "@/lib/queryClient";
import { useDownloads } from "@/stores/downloads";
import { useToasts } from "@/stores/toasts";
import type { Browser, SignInMethod, SignInOutcome } from "@/types";

/** A method as one string, for choosing it in a facet: "window", "browser:firefox", "file". */
export type MethodId = string;

export function methodId(method: SignInMethod): MethodId {
  return method.type === "browser" ? `browser:${method.browser}` : method.type;
}

export function methodOf(id: MethodId): SignInMethod {
  if (id.startsWith("browser:")) return { type: "browser", browser: id.slice("browser:".length) };
  return id === "file" ? { type: "file" } : { type: "window" };
}

/** Every way to sign in, in the order offered: Siphon's window, the browsers here, a file. */
export function methodIds(browsers: Browser[]): MethodId[] {
  return ["window", ...browsers.map((browser) => `browser:${browser.id}`), "file"];
}

interface SignInState {
  /** The browsers on this computer, once looked for. */
  browsers: Browser[] | null;
  loadBrowsers: () => Promise<void>;
  /** The methods that didn't work for each site this session, so the next suggestion skips them. */
  failed: Record<string, MethodId[]>;
  /** The site being signed in to, while it lasts. */
  busy: string | null;
  /** Signs in to `site` with `method`, checked on `url`; afterwards retries what the sign-in unlocks. */
  run: (url: string, site: string, method: SignInMethod) => Promise<SignInOutcome | null>;
}

export const useSignIn = create<SignInState>((set, get) => ({
  browsers: null,
  loadBrowsers: async () => {
    if (get().browsers) return;
    set({ browsers: await ipc.signInBrowsers().catch(() => []) });
  },
  failed: {},
  busy: null,
  run: async (url, site, method) => {
    if (get().busy) return null;
    set({ busy: site });
    try {
      const outcome = await ipc.signIn(url, method);
      if (outcome.outcome === "refused") {
        const failed = get().failed[site] ?? [];
        set({ failed: { ...get().failed, [site]: [...new Set([...failed, methodId(method)])] } });
      }
      if (outcome.outcome === "signed_in") signedIn(outcome.site, outcome.tip);
      return outcome;
    } catch (error) {
      useToasts.getState().show({ id: "sign-in", title: "Couldn't sign in", description: toAppError(error).message });
      return null;
    } finally {
      set({ busy: null });
    }
  },
}));

/**
 * The suggested way to sign in to a site: Siphon’s window, unless it didn’t
 * work there this session; then Firefox if it's here (the browser whose
 * cookies Siphon can reliably read), then any other method not yet tried.
 */
export function suggestedMethod(browsers: Browser[], failed: MethodId[]): MethodId {
  const order = ["window", ...(browsers.some((b) => b.id === "firefox") ? ["browser:firefox"] : []), "file"];
  const all = methodIds(browsers);
  return [...order, ...all].find((id) => !failed.includes(id)) ?? "window";
}

/**
 * Signs in the suggested way, for a place too small to offer a choice (a
 * row in the download list). A refusal is shown as a toast.
 */
export async function signInSuggested(url: string, site: string) {
  const state = useSignIn.getState();
  await state.loadBrowsers();
  const { browsers, failed } = useSignIn.getState();
  const method = suggestedMethod(browsers ?? [], failed[site] ?? []);
  const outcome = await state.run(url, site, methodOf(method));
  if (outcome?.outcome === "refused") {
    useToasts.getState().show({ id: "sign-in", title: outcome.short, description: outcome.reason });
  }
}

/** What a new sign-in unlocks: failed downloads of the site are tried again, and Settings' list refreshes. */
function signedIn(site: string, tip: boolean) {
  const downloads = useDownloads.getState();
  for (const id of downloads.order) {
    const download = downloads.byId[id];
    if (download?.status === "failed" && download.error?.sign_in?.site === site) downloads.retry(id);
  }
  void queryClient.invalidateQueries({ queryKey: ["signed-in-sites"] });
  useToasts.getState().show({
    id: "sign-in",
    title: `Signed in to ${site}`,
    // Once, the first time: an account that downloads a lot can be flagged.
    description: tip ? "Tip: downloading a lot with your account can get it flagged, so a second account is safer." : undefined,
  });
}
