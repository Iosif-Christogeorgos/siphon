import { AnimatePresence } from "motion/react";
import { useEffect } from "react";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { TitleBar } from "@/components/TitleBar";
import { Toaster } from "@/components/Toaster";
import { Home } from "@/features/home/Home";
import { QueueSheet } from "@/features/queue/QueueSheet";
import { SettingsSheet } from "@/features/settings/SettingsSheet";
import { SetupScreen } from "@/features/setup/SetupScreen";
import { onDownloadEvent, onToolStatus, onToolUpdated } from "@/lib/ipc";
import { TOOL_NAMES } from "@/lib/tools";
import { useAppUpdate } from "@/stores/appUpdate";
import { selectAllUsable, useBinaries } from "@/stores/binaries";
import { useDownloads } from "@/stores/downloads";
import { useToasts } from "@/stores/toasts";

export function App() {
  const loaded = useBinaries((state) => state.tools !== null);
  const usable = useBinaries(selectAllUsable);

  // Wire backend events into the stores, then check the managed binaries.
  useEffect(() => {
    const subscriptions = [
      onToolStatus(useBinaries.getState().apply),
      onDownloadEvent(useDownloads.getState().apply),
      // A subtle note when a tool was updated in the background.
      onToolUpdated((notice) =>
        useToasts.getState().show({
          id: `updated-${notice.tool}`,
          title: `${TOOL_NAMES[notice.tool]} updated to ${notice.to}`,
          description: notice.summary,
        }),
      ),
    ];
    void useBinaries
      .getState()
      .sync()
      .then(() => {
        // In "ask first" mode, newer versions are only reported: offer them.
        const waiting = Object.values(useBinaries.getState().tools ?? {}).filter((status) => status.available);
        const first = waiting[0];
        if (!first?.available) return;
        useToasts.getState().show({
          id: "updates-available",
          title:
            waiting.length === 1
              ? `${TOOL_NAMES[first.tool]} ${first.available.version} is available`
              : `${waiting.length} updates are available`,
          description: first.available.summary,
          action: { label: "Update", run: () => void useBinaries.getState().sync(true) },
        });
      });
    // Siphon itself: look at launch and once a day, and always ask before restarting.
    const checkApp = () =>
      void useAppUpdate
        .getState()
        .check()
        .then(() => {
          const { available, install } = useAppUpdate.getState();
          if (!available) return;
          useToasts.getState().show({
            id: "app-update",
            title: `Siphon ${available} is available`,
            description: "Updating takes a few seconds and restarts the app.",
            action: { label: "Restart to update", run: () => void install() },
          });
        });
    checkApp();
    const daily = window.setInterval(checkApp, 24 * 60 * 60 * 1000);
    return () => {
      window.clearInterval(daily);
      for (const subscription of subscriptions) void subscription.then((unlisten) => unlisten());
    };
  }, []);

  return (
    <div className="relative flex h-full flex-col">
      {/* A faint wash of the brand colour behind the top of the window. */}
      <div className="pointer-events-none absolute inset-x-0 top-0 h-80 bg-[radial-gradient(60%_100%_at_50%_0%,var(--primary-soft),transparent)]" />
      <TitleBar />
      <main className="relative min-h-0 flex-1">
        <ErrorBoundary>
          <AnimatePresence mode="wait">
            {!loaded ? null : usable ? <Home key="home" /> : <SetupScreen key="setup" />}
          </AnimatePresence>
          {/* Inside the boundary too: a panel that fails must not blank the window. */}
          <QueueSheet />
          <SettingsSheet />
        </ErrorBoundary>
      </main>
      <Toaster />
    </div>
  );
}
