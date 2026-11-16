import { AnimatePresence } from "motion/react";
import { useEffect } from "react";
import { TitleBar } from "@/components/TitleBar";
import { Home } from "@/features/home/Home";
import { SetupScreen } from "@/features/setup/SetupScreen";
import { onDownloadEvent, onToolStatus } from "@/lib/ipc";
import { selectAllUsable, useBinaries } from "@/stores/binaries";
import { useDownloads } from "@/stores/downloads";

export function App() {
  const loaded = useBinaries((state) => state.tools !== null);
  const usable = useBinaries(selectAllUsable);

  // Wire backend events into the stores, then check the managed binaries.
  useEffect(() => {
    const subscriptions = [
      onToolStatus(useBinaries.getState().apply),
      onDownloadEvent(useDownloads.getState().apply),
    ];
    void useBinaries.getState().sync();
    return () => {
      for (const subscription of subscriptions) void subscription.then((unlisten) => unlisten());
    };
  }, []);

  return (
    <div className="relative flex h-full flex-col">
      {/* A faint wash of the brand colour behind the top of the window. */}
      <div className="pointer-events-none absolute inset-x-0 top-0 h-80 bg-[radial-gradient(60%_100%_at_50%_0%,var(--primary-soft),transparent)]" />
      <TitleBar />
      <main className="relative min-h-0 flex-1">
        <AnimatePresence mode="wait">
          {!loaded ? null : usable ? <Home key="home" /> : <SetupScreen key="setup" />}
        </AnimatePresence>
      </main>
    </div>
  );
}
