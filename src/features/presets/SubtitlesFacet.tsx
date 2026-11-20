import { ChevronDown } from "lucide-react";
import { Select as SelectPrimitive } from "radix-ui";
import { SelectContent, SelectItem } from "@/components/ui/select";
import { Hint } from "@/components/ui/tooltip";
import { languageName, pickTrack, playlistLanguages, subtitleLanguages } from "@/lib/languages";
import { cn } from "@/lib/utils";
import { useChoices } from "@/stores/choices";
import type { SubtitleTrack } from "@/types";

/** Radix selects need a non-empty value; this one stands for no subtitles. */
const OFF = "off";

/**
 * "Subtitles  English ⌄": the language subtitles come in, or Off. A list
 * rather than a facet, since a video can have dozens (a TED talk has 64);
 * typing a language's first letters jumps to it. Siphon remembers the
 * language, not this video's track, so the choice carries over to every
 * video that has it. Looks like a folded facet, and fades with the others
 * while one of them is unfolded.
 */
export function SubtitlesFacet({
  tracks,
  disabled,
  unavailable,
}: {
  /** The video's tracks, or null for a playlist, whose videos each have their own. */
  tracks: SubtitleTrack[] | null;
  disabled: boolean;
  /** Why this choice can't take subtitles: shown greyed out, with this as its hint. */
  unavailable?: string;
}) {
  const wanted = useChoices((state) => state.choices?.subtitles ?? null);
  const options = tracks ? subtitleLanguages(tracks) : playlistLanguages(wanted);
  const used = tracks && wanted ? pickTrack(tracks, wanted) : undefined;
  // The language of the track that will be used, or the one asked for when this video lacks it.
  const value = wanted === null ? OFF : (used?.lang ?? wanted);
  const missing = wanted !== null && tracks !== null && used === undefined;
  const off = unavailable !== undefined;

  const trigger = (
    <SelectPrimitive.Trigger
      aria-label="Subtitles"
      disabled={disabled || off}
      className={cn(
        "flex h-9 items-center gap-2 rounded-xl border bg-card pr-2 pl-3 whitespace-nowrap outline-none",
        "shadow-[0_1px_2px_color-mix(in_srgb,var(--shadow-color)_6%,transparent)] transition-[border-color,opacity] duration-150",
        "hover:border-border-strong focus-visible:ring-[3px] focus-visible:ring-ring/40 data-[state=open]:border-border-strong",
        "in-data-facet-open:opacity-0 disabled:opacity-50",
      )}
    >
      <span className="text-xs text-subtle-foreground">Subtitles</span>
      <span className={cn("text-[13px] font-medium", (missing || off) && "text-muted-foreground")}>
        {off ? "Off" : <SelectPrimitive.Value />}
      </span>
      <ChevronDown className="-ml-0.5 size-3.5 text-subtle-foreground" />
    </SelectPrimitive.Trigger>
  );

  return (
    <SelectPrimitive.Root
      value={off ? OFF : value}
      onValueChange={(next) => useChoices.getState().setSubtitles(next === OFF ? null : next)}
      disabled={disabled || off}
    >
      {off ? (
        <Hint label={unavailable}>
          {/* A disabled button gets no pointer events: the hint needs something that does. */}
          <span className="inline-flex">{trigger}</span>
        </Hint>
      ) : (
        trigger
      )}
      <SelectContent className="max-h-80 min-w-56">
        <SelectItem value={OFF}>Off</SelectItem>
        {missing && (
          <SelectItem value={wanted} hint="Not in this video">
            {languageName(wanted)}
          </SelectItem>
        )}
        {options.map((option) => (
          <SelectItem key={option.lang} value={option.lang} hint={option.automatic ? "Automatic" : undefined}>
            {option.name}
          </SelectItem>
        ))}
      </SelectContent>
    </SelectPrimitive.Root>
  );
}
