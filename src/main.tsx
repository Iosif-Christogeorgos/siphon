import { QueryClientProvider } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SignInBar } from "@/features/sign-in/SignInBar";
import { queryClient } from "@/lib/queryClient";
import { App } from "./App";
import "./index.css";
// Sets the chosen theme before anything is drawn.
import "@/stores/theme";

/** The sign-in window's bar is this same app, opened at `#sign-in` (see `sessions/sign_in.rs`). */
const isSignInBar = window.location.hash === "#sign-in";

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      {/* "user" makes every animation honour the OS "reduce motion" setting. */}
      <MotionConfig reducedMotion="user">
        <TooltipProvider>
          {isSignInBar ? <SignInBar /> : <App />}
        </TooltipProvider>
      </MotionConfig>
    </QueryClientProvider>
  </StrictMode>,
);

// The window starts hidden (tauri.conf.json) so there is no white flash;
// reveal it once the first frame has been painted.
if (!isSignInBar) requestAnimationFrame(() => void getCurrentWindow().show());
