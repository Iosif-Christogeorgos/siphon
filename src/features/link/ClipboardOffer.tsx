import { ArrowUpRight, Clipboard, X } from "lucide-react";
import { displayHost } from "@/lib/url";

/** "There's a link on your clipboard" chip shown under the paste field. */
export function ClipboardOffer({
  url,
  onAccept,
  onDismiss,
}: {
  url: string;
  onAccept: () => void;
  onDismiss: () => void;
}) {
  return (
    // The padding gives the chip's shadow room inside the collapsing wrapper.
    <div className="flex justify-center px-4 pt-3 pb-2">
      <div className="flex h-9 max-w-full items-center rounded-full border bg-card pr-1 shadow-card">
        <button
          type="button"
          onClick={onAccept}
          className="group flex h-full min-w-0 items-center gap-2 rounded-l-full pr-2 pl-3.5 text-[13px]"
        >
          <Clipboard className="size-3.5 shrink-0 text-primary" />
          <span className="shrink-0 text-muted-foreground transition-colors group-hover:text-foreground">
            Use the link on your clipboard
          </span>
          <span className="truncate font-medium">{displayHost(url)}</span>
          <ArrowUpRight className="size-3.5 shrink-0 text-subtle-foreground transition-[color,transform] duration-200 group-hover:translate-x-0.5 group-hover:-translate-y-0.5 group-hover:text-primary" />
        </button>
        <button
          type="button"
          aria-label="Dismiss"
          onClick={onDismiss}
          className="grid size-7 shrink-0 place-items-center rounded-full text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      </div>
    </div>
  );
}
