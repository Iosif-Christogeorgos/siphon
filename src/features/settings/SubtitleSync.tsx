import { ArrowDownToLine, CircleAlert, Cpu, LoaderCircle, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useSubSync } from "@/stores/subsync";
import type { SubSyncStatus } from "@/types";
import { Card, Row, Status } from "./parts";

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
        <div className="space-y-1">
          <Status tone="off" label="Not available">
            Needs an NVIDIA graphics card with 8 GB of memory
          </Status>
          {status.gpu && (
            <p className="flex items-center gap-1.5 pl-3 text-xs text-subtle-foreground">
              <Cpu className="size-3 shrink-0" />
              {status.gpu}
            </p>
          )}
        </div>
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
        <Status tone="off" label="Not downloaded">
          {download} to download, {disk} on disk. Runs on your {status.gpu}.
        </Status>
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
          <div className="h-1.5 overflow-hidden rounded-full bg-well shadow-[inset_0_0_0_1px_var(--border)]">
            <div
              className="h-full rounded-full bg-primary shadow-[0_0_8px_var(--primary-glow)] transition-[width] duration-500 ease-out"
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
          className={cn("h-7 px-2.5", confirming && "bg-destructive-soft text-destructive hover:bg-destructive/15 hover:text-destructive")}
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
        <div className="space-y-2">
          <Status tone="on" label="Ready">
            {status.gpu}
          </Status>
          <p className="text-xs leading-normal text-pretty text-muted-foreground">
            In a film’s or a season’s tracks, press <span className="font-medium text-foreground">Sync to speech</span>{" "}
            beside subtitles in text, the film’s own or ones you add.
          </p>
          {removeError && <p className="text-xs text-destructive">{removeError}</p>}
        </div>
      );
      break;
  }

  return (
    <Card>
      <Row title="Sync to speech" hint={hint} control={control} stacked>
        {body}
      </Row>
    </Card>
  );
}
