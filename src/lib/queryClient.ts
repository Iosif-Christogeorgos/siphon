import { QueryClient } from "@tanstack/react-query";

/** One cache for the whole interface, also reachable from stores (to refresh a list after a change). */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Metadata lookups are slow and their failures are deterministic
      // (unsupported link, DRM, …), so never retry or refetch behind the user's back.
      retry: false,
      refetchOnWindowFocus: false,
      staleTime: 5 * 60 * 1000,
    },
  },
});
