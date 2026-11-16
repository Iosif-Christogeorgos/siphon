import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import { App } from "./App";
import "./index.css";

const queryClient = new QueryClient({
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

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      {/* "user" makes every animation honour the OS "reduce motion" setting. */}
      <MotionConfig reducedMotion="user">
        <TooltipProvider>
          <App />
        </TooltipProvider>
      </MotionConfig>
    </QueryClientProvider>
  </StrictMode>,
);

// The window starts hidden (tauri.conf.json) so there is no white flash;
// reveal it once the first frame has been painted.
requestAnimationFrame(() => void getCurrentWindow().show());
