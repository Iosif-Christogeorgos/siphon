import { ArrowDownToLine, CircleAlert, Cpu, LoaderCircle, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useSubSync } from "@/stores/subsync";
import type { SubSyncStatus } from "@/types";
import { Card, Row } from "./parts";

const STEPS: Record<NonNullable<SubSyncStatus["step"]>, string> = {
  tools: "Getting the installer",
  python: "Setting up a private Python",
  libraries: "Downloading the AI libraries",
  models: "Downloading the speech models",
};

/**
 * Subtitle sync's optional download (docs/subtitle-sync.md): offered only on
 * a PC with an NVIDIA graphics card, made only when asked for, and taken away
 * again here. Until it is there, **Sync to speech** in the tracks card is locked.
 */
export function SubtitleSync() {
  const status = useSubSync((state) => state.status);
  const install = useSubSync((state) => state.install);
  const remove = useSubSync((state) => state.remove);
  const [confirming, setConfirming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  if (!status) return null;

  const download = formatBytes(status.download_bytes);
  const disk = formatBytes(status.disk_bytes);
  const hint =
    "Gives every line of a subtitle file the moment its words are spoken in the film, " +
    "with speech recognition on your graphics card. Download it only if you need it.";

  let control = null;
  let body = null;
  switch (status.state) {
    case "no_gpu":
      body = (
        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <Cpu className="mt-px size-3.5 shrink-0" />
          <span>Needs an NVIDIA graphics card with 8 GB of memory. {status.gpu}</span>
        </p>
      );
      break;
    case "absent":
      control = (
        <Button size="sm" onClick={() => void install()}>
          <ArrowDownToLine />
          Download
        </Button>
      );
      body = (
        <p className="text-xs text-muted-foreground">
          {download} to download, {disk} on disk. Runs on your {status.gpu}.
        </p>
      );
      break;
    case "installing": {
      const progress = status.progress ?? 0;
      body = (
        <div className="space-y-1.5" aria-live="polite">
          <div className="flex items-center justify-between gap-3 text-xs">
            <span className="flex min-w-0 items-center gap-1.5 truncate text-muted-foreground">
              <LoaderCircle className="size-3.5 shrink-0 animate-spin text-primary" />
              {status.step ? STEPS[status.step] : "Starting"}…
            </span>
            <span className="shrink-0 font-medium tabular-nums">{Math.round(progress * 100)}%</span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-secondary">
            <div
              className="h-full rounded-full bg-primary transition-[width] duration-500 ease-out"
              style={{ width: `${Math.max(2, progress * 100)}%` }}
            />
          </div>
          <p className="text-xs text-subtle-foreground">
            Carries on from where it stopped if Siphon closes or the connection drops.
          </p>
        </div>
      );
      break;
    }
    case "failed":
      control = (
        <Button size="sm" onClick={() => void install()}>
          <ArrowDownToLine />
          Try again
        </Button>
      );
      body = (
        <p className="flex items-start gap-2 rounded-lg bg-destructive-soft px-2.5 py-2 text-xs text-destructive">
          <CircleAlert className="mt-px size-3.5 shrink-0" />
          <span>{status.error ?? "The download didn't finish."} Trying again carries on from where it stopped.</span>
        </p>
      );
      break;
    case "ready":
      control = (
        <Button
          size="sm"
          variant="ghost"
          className={cn("h-7 px-2.5", confirming && "text-destructive")}
          disabled={removing}
          onClick={async () => {
            if (!confirming) {
              setConfirming(true);
              return;
            }
            setRemoving(true);
            setRemoveError(await remove());
            setRemoving(false);
            setConfirming(false);
          }}
          onBlur={() => setConfirming(false)}
        >
          {removing ? <LoaderCircle className="size-3.5 animate-spin" /> : <Trash2 className="size-3.5" />}
          {confirming ? `Remove ${disk}?` : "Remove"}
        </Button>
      );
      body = (
        <p className="text-xs text-muted-foreground">
          <span className="font-medium text-success">Ready.</span> Runs on your {status.gpu}. In a film’s tracks, open
          an added SRT file and press <span className="font-medium text-foreground">Sync to speech</span>.
          {removeError && <span className="mt-1 block text-destructive">{removeError}</span>}
        </p>
      );
      break;
  }

  return (
    <Card>
      <Row title="Sync to speech" hint={hint} control={control}>
        {body}
      </Row>
    </Card>
  );
}
