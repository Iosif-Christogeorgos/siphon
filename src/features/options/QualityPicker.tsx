import { RefreshCw } from "lucide-react";
import { Chip } from "@/components/ui/chip";
import { Hint } from "@/components/ui/tooltip";
import { SHARE_LIMITS_MB, useOptions } from "@/stores/options";
import type { PlanSummary } from "@/types";

/** Below this, a stream is a preview thumbnail rather than a real choice. */
const MIN_LINES = 144;
const MAX_CHOICES = 7;

/**
 * The second row of choices under the presets: resolutions for most presets,
 * upload limits for the Share preset, nothing for audio.
 */
export function QualityPicker({ plan, disabled }: { plan: PlanSummary | undefined; disabled: boolean }) {
  const preset = useOptions((state) => state.preset);
  const maxLines = useOptions((state) => state.maxLines);
  const sizeLimitMb = useOptions((state) => state.sizeLimitMb);
  const setMaxLines = useOptions((state) => state.setMaxLines);
  const setSizeLimit = useOptions((state) => state.setSizeLimit);

  if (preset === "share") {
    return (
      <Row label="Fit under">
        {SHARE_LIMITS_MB.map((megabytes) => (
          <Chip
            key={megabytes}
            group="size"
            tone="soft"
            selected={sizeLimitMb === megabytes}
            onSelect={() => setSizeLimit(megabytes)}
            disabled={disabled}
          >
            {megabytes} MB
          </Chip>
        ))}
      </Row>
    );
  }

  const qualities = (plan?.qualities ?? [])
    .filter((quality) => quality.lines >= MIN_LINES)
    .slice(0, MAX_CHOICES);
  if (qualities.length < 2) return null;

  return (
    <Row label="Quality">
      <Chip
        group="quality"
        tone="soft"
        selected={maxLines === null}
        onSelect={() => setMaxLines(null)}
        disabled={disabled}
      >
        Auto
      </Chip>
      {qualities.map(({ lines, converts }) => (
        <Chip
          key={lines}
          group="quality"
          tone="soft"
          selected={maxLines === lines}
          onSelect={() => setMaxLines(lines)}
          disabled={disabled}
        >
          {lines}p
          {converts && (
            <Hint label="Needs converting for this device (slower)">
              <RefreshCw className="size-3! opacity-70" />
            </Hint>
          )}
        </Chip>
      ))}
    </Row>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-14 shrink-0 text-xs text-subtle-foreground">{label}</span>
      <div role="radiogroup" aria-label={label} className="flex flex-wrap gap-0.5">
        {children}
      </div>
    </div>
  );
}
