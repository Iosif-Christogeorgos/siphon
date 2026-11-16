import { TitleBar } from "@/components/TitleBar";

export function App() {
  return (
    <div className="relative flex h-full flex-col">
      {/* A faint wash of the brand colour behind the top of the window. */}
      <div className="pointer-events-none absolute inset-x-0 top-0 h-80 bg-[radial-gradient(60%_100%_at_50%_0%,var(--primary-soft),transparent)]" />
      <TitleBar />
      <main className="relative min-h-0 flex-1" />
    </div>
  );
}
