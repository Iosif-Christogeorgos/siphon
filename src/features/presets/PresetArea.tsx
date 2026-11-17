import { RefreshCw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import type { ReactNode } from "react";
import { Chip } from "@/components/ui/chip";
import { onRadioKeys } from "@/components/ui/radio-keys";
import { Segmented } from "@/components/ui/segmented";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatBytesRange } from "@/lib/format";
import { iconFor } from "@/lib/icons";
import { spring } from "@/lib/motion";
import { currentTarget, useChoices, useRegistry } from "@/stores/choices";
import type { Choices, PlanSummary, QualityOption, Registry, Target } from "@/types";

/** Resolutions offered for a playlist, where no single video's formats are known. */
const PLAYLIST_LADDER = [2160, 1440, 1080, 720, 480, 360];

/**
 * Where the download is going: the group (Watch, Edit, …), the target in it,
 * the options that target has, the quality row and the user's own presets.
 * Everything comes from the backend's registry; nothing is defined here.
 */
export function PresetArea({
  summary,
  disabled,
  playlist = false,
}: {
  /** The plan for the current choice; gives the quality row its options and sizes. */
  summary: PlanSummary | undefined;
  disabled: boolean;
  /** A playlist: the quality row offers a fixed ladder, without sizes. */
  playlist?: boolean;
}) {
  const registry = useRegistry();
  const choices = useChoices((state) => state.choices);
  const chooseGroup = useChoices((state) => state.chooseGroup);
  if (!registry || !choices) return <div aria-hidden className="skeleton h-[4.5rem] rounded-xl" />;

  const presetActive = choices.active.kind === "preset";
  const group = registry.groups.find((candidate) => candidate.id === choices.group) ?? registry.groups[0];
  const target = currentTarget(registry, choices);
  if (!group || !target) return null;

  return (
    // No margin class here: it would cancel the parent's `space-y`, and Tailwind's base styles already zero it.
    <fieldset disabled={disabled} className="min-w-0 space-y-2 border-0 p-0">
      <Segmented
        label="What it’s for"
        group="preset-group"
        size="md"
        value={presetActive ? group.id : null}
        onChange={chooseGroup}
        options={registry.groups.map((candidate) => {
          const Icon = iconFor(candidate.icon);
          return { id: candidate.id, label: candidate.label, icon: <Icon /> };
        })}
      />

      {presetActive && (
        <>
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.div
              key={group.id}
              initial={{ opacity: 0, y: -4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              transition={spring}
            >
              {group.targets.length > 1 ? (
                <div
                  role="radiogroup"
                  aria-label={group.label}
                  onKeyDown={onRadioKeys}
                  className="flex flex-wrap gap-1"
                >
                  {group.targets.map((candidate) => {
                    const Icon = iconFor(candidate.icon);
                    return (
                      <Chip
                        key={candidate.id}
                        group="target"
                        selected={candidate.id === target.id}
                        onSelect={() => useChoices.getState().chooseTarget(group.id, candidate.id)}
                      >
                        <Icon />
                        {candidate.label}
                      </Chip>
                    );
                  })}
                </div>
              ) : (
                <p className="flex h-8 items-center px-1 text-[13px] text-pretty text-muted-foreground">
                  {group.description}
                </p>
              )}
            </motion.div>
          </AnimatePresence>
          <Extras target={target} choices={choices} />
          <QualityRow registry={registry} target={target} choices={choices} summary={summary} playlist={playlist} />
        </>
      )}
      <SavedRow choices={choices} />
    </fieldset>
  );
}

/** The secondary choices a target has, each on its own compact row. */
function Extras({ target, choices }: { target: Target; choices: Choices }) {
  const { setTier, setSmoothEditing, setMusicFormat } = useChoices.getState();
  return (
    <>
      {target.extras.includes("tier") && target.tiers.length > 0 && (
        <Row label="Limit">
          {target.tiers.map((tier) => (
            <Chip
              key={tier.id}
              group={`tier-${target.id}`}
              tone="soft"
              selected={(choices.tiers[target.id] ?? target.tiers[0]?.id) === tier.id}
              onSelect={() => setTier(target.id, tier.id)}
            >
              {tier.label}
            </Chip>
          ))}
        </Row>
      )}
      {target.extras.includes("smooth_editing") && (
        <Row label="Files">
          <Chip group="smooth" tone="soft" selected={!choices.smooth_editing} onSelect={() => setSmoothEditing(false)}>
            Standard
          </Chip>
          <Hint label="ProRes: every frame stands on its own, so scrubbing is instant. About ten times larger.">
            <span>
              <Chip group="smooth" tone="soft" selected={choices.smooth_editing} onSelect={() => setSmoothEditing(true)}>
                Smoothest editing (large files)
              </Chip>
            </span>
          </Hint>
        </Row>
      )}
      {target.extras.includes("music_format") && (
        <Row label="Format">
          <Chip group="music" tone="soft" selected={choices.music_format === "mp3"} onSelect={() => setMusicFormat("mp3")}>
            MP3
          </Chip>
          <Chip group="music" tone="soft" selected={choices.music_format === "m4a"} onSelect={() => setMusicFormat("m4a")}>
            M4A (AAC)
          </Chip>
        </Row>
      )}
    </>
  );
}

/**
 * "Best", then the resolutions the video really has (or bitrates for Music),
 * each with its estimated size in small print.
 */
function QualityRow({
  registry,
  target,
  choices,
  summary,
  playlist,
}: {
  registry: Registry;
  target: Target;
  choices: Choices;
  summary: PlanSummary | undefined;
  playlist: boolean;
}) {
  const bitrate = target.quality === "bitrate";
  const options: QualityOption[] = playlist
    ? (bitrate ? registry.music_bitrates.toReversed() : target.quality === "resolution" ? PLAYLIST_LADDER : []).map(
        (value) => ({ value, converts: false, estimated_size: null, size_range: null }),
      )
    : summary?.quality === target.quality
      ? summary.options
      : [];
  if (target.quality === "none" || options.length < (bitrate ? 1 : 2)) return null;

  const selected = bitrate ? choices.music_kbps : choices.quality;
  // Worth marking only when some choices convert and others don't.
  const mixed = options.some((option) => option.converts) && options.some((option) => !option.converts);
  const set = (value: number | null) => useChoices.getState().setQuality(value, bitrate);
  // With sizes under the choices, Best grows to the same height so the row stays even.
  const twoLines = options.some((option) => option.estimated_size !== null);
  return (
    <Row label="Quality">
      <Chip
        group="quality"
        tone="soft"
        selected={selected === null}
        onSelect={() => set(null)}
        className={twoLines ? "h-auto self-stretch rounded-xl" : undefined}
      >
        Best
      </Chip>
      {options.map(({ value, converts, estimated_size, size_range }) => (
        <Chip
          key={value}
          group="quality"
          tone="soft"
          selected={selected === value}
          onSelect={() => set(value)}
          // Two short lines: the choice, and its size in small print under it.
          className={estimated_size !== null ? "h-auto rounded-xl px-1.5 py-1" : undefined}
        >
          <span className="flex flex-col items-center leading-tight">
            <span className="flex items-center gap-1">
              {bitrate ? `${value} kbps` : `${value}p`}
              {converts && mixed && (
                <Hint label="Converted for this target (slower)">
                  <RefreshCw className="size-3! opacity-70" />
                </Hint>
              )}
            </span>
            {estimated_size !== null && (
              <span className="text-[10.5px] font-normal text-subtle-foreground tabular-nums">
                {size_range ? formatBytesRange(size_range[0], size_range[1]) : formatBytes(estimated_size)}
              </span>
            )}
          </span>
        </Chip>
      ))}
    </Row>
  );
}

/** The user's own saved presets, when there are any. */
function SavedRow({ choices }: { choices: Choices }) {
  if (choices.saved.length === 0) return null;
  const active = choices.active.kind === "saved" ? choices.active.id : null;
  return (
    <Row label="My presets">
      {choices.saved.map((preset) => (
        <Chip
          key={preset.id}
          group="saved"
          tone="soft"
          selected={preset.id === active}
          onSelect={() => useChoices.getState().chooseSaved(preset.id)}
        >
          {preset.name}
        </Chip>
      ))}
    </Row>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-[4.5rem] shrink-0 text-xs text-subtle-foreground">{label}</span>
      <div role="radiogroup" aria-label={label} onKeyDown={onRadioKeys} className="flex flex-wrap gap-1">
        {children}
      </div>
    </div>
  );
}
