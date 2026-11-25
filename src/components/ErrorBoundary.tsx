import { RotateCw } from "lucide-react";
import { Component, type ReactNode } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";

/**
 * Last line of defence: if a screen throws while rendering, show a readable
 * message with a way back instead of an empty window.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="mx-auto flex h-full max-w-[640px] flex-col justify-center px-6 pb-16">
        <ErrorCard
          error={{
            kind: "internal",
            message: "Siphon hit a problem drawing this screen. What’s already running carries on.",
            details: error.stack ?? error.message,
            sign_in: null,
          }}
          action={
            <Button variant="secondary" size="sm" onClick={() => this.setState({ error: null })}>
              <RotateCw /> Reload
            </Button>
          }
        />
      </div>
    );
  }
}
