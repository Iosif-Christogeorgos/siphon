import { RefreshCw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { Facet } from "@/components/ui/facet";
import { Segmented } from "@/components/ui/segmented";
import { Hint } from "@/components/ui/tooltip";
import { formatBytes, formatBytesRange } from "@/lib/format";
import { iconFor } from "@/lib/icons";
import { spring } from "@/lib/motion";
import { currentTarget, useChoices, useRegistry } from "@/stores/choices";
import type { Choices, PlanSummary, QualityOption, Registry, SubtitleTrack, Target } from "@/types";
import { SubtitlesFacet } from "./SubtitlesFacet";

/** Resolutions offered for a playlist, where no single video's formats are known. */
const PLAYLIST_LADDER = [2160, 1440, 1080, 720, 480, 360];

/** The group that makes a file from the whole video: not for a stream as it airs. */
const FINISHED_ONLY = "share";

/**
 * Where the download is going: the group (Max quality, Watch, …) on top, then
 * one row of facets: the target in it, the options that target has, the
 * quality and the user's own presets. Each facet shows only its current value
 * until it is pointed at. Everything comes from the backend's registry.
 */
export function PresetArea({
  summary,
  disabled,
  playlist = false,
  live = false,
  from,
  subtitles,
}: {
  /** The plan for the current choice; gives the quality facet its options and sizes. */
  summary: PlanSummary | undefined;
  disabled: boolean;
  /** A playlist: the quality facet offers a fixed ladder, without sizes. */
  playlist?: boolean;
  /** A live stream: Share waits for the finished recording. */
  live?: boolean;
  /** Where the site keeps what already aired: record from now, or from the start. */
  from?: { start: boolean; onChange: (start: boolean) => void };
  /** The video's subtitle tracks, or "playlist" for videos that each have their own; none for a stream as it airs. */
  subtitles?: SubtitleTrack[] | "playlist";
}) {
  const registry = useRegistry();
  const choices = useChoices((state) => state.choices);
  const chooseGroup = useChoices((state) => state.chooseGroup);
  if (!registry || !choices) return <div aria-hidden className="skeleton h-[5.25rem] rounded-xl" />;

  const presetActive = choices.active.kind === "preset";
  const group = registry.groups.find((candidate) => candidate.id === choices.group) ?? registry.groups[0];
  const target = currentTarget(registry, choices);
  if (!group || !target) return null;
  // Subtitles: for a video that has some, or a playlist; never for a sound-only preset.
  const hasSubtitles = subtitles === "playlist" || (subtitles !== undefined && subtitles.length > 0);
  const showSubtitles = hasSubtitles && !(presetActive && group.id === "audio");
  const audioOnly = !presetActive && (choices.active.kind === "custom" ? choices.custom.content === "audio_only" : false);
  const noSubtitles =
    summary?.subtitle_delivery === null || (summary === undefined && audioOnly)
      ? "Audio files can't hold subtitles."
      : undefined;

  return (
    // No margin class here: it would cancel the parent's `space-y`, and Tailwind's base styles already zero it.
    <fieldset disabled={disabled} className="relative min-w-0 border-0 p-0">
      <Segmented
        label="What it’s for"
        group="preset-group"
        size="md"
        value={presetActive ? group.id : null}
        onChange={chooseGroup}
        options={registry.groups.map((candidate) => {
          const Icon = iconFor(candidate.icon);
          const later = live && candidate.id === FINISHED_ONLY;
          return {
            id: candidate.id,
            label: candidate.label,
            icon: <Icon />,
            hint: later ? `${candidate.label} becomes available on the finished recording.` : candidate.description,
            disabled: later,
          };
        })}
      />

      <AnimatePresence mode="popLayout" initial={false}>
        <motion.div
          // A new group brings its own facets: they rise in together.
          key={presetActive ? group.id : choices.active.kind}
          data-facet-row
          // `empty:hidden` takes the margin too, so a group without options (Max quality) leaves no gap.
          className="relative mt-3 flex flex-wrap items-center gap-2 empty:hidden"
          initial={{ opacity: 0, y: -4 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, transition: { duration: 0.1 } }}
          transition={spring}
        >
          {presetActive && group.targets.length > 1 && (
            <Facet
              label="For"
              group="target"
              value={target.id}
              disabled={disabled}
              onChange={(id) => useChoices.getState().chooseTarget(group.id, id)}
              options={group.targets.map((candidate) => {
                const Icon = iconFor(candidate.icon);
                return { id: candidate.id, label: candidate.label, icon: <Icon /> };
              })}
            />
          )}
          {presetActive && <Extras target={target} choices={choices} disabled={disabled} />}
          {presetActive && (
            <QualityFacet
              registry={registry}
              target={target}
              choices={choices}
              summary={summary}
              playlist={playlist}
              disabled={disabled}
            />
          )}
          {from && (
            <Facet
              label="From"
              group="from"
              value={from.start ? "start" : "now"}
              onChange={(id) => from.onChange(id === "start")}
              disabled={disabled}
              options={[
                { id: "now", label: "Now", detail: "As it airs" },
                { id: "start", label: "The start", detail: "Catches up, then live" },
              ]}
            />
          )}
          {showSubtitles && (
            <SubtitlesFacet
              tracks={subtitles === "playlist" ? null : (subtitles ?? null)}
              disabled={disabled}
              unavailable={noSubtitles}
            />
          )}
          <SavedFacet choices={choices} disabled={disabled} />
        </motion.div>
      </AnimatePresence>
    </fieldset>
  );
}

/** "Free · 20 MB" → the name, and the limit as small print. */
function splitTier(label: string): { label: string; detail?: string } {
  const [name, limit] = label.split(" · ");
  return { label: name ?? label, detail: limit };
}

/** The secondary choices a target has. */
function Extras({ target, choices, disabled }: { target: Target; choices: Choices; disabled: boolean }) {
  const { setTier, setMusicFormat } = useChoices.getState();
  return (
    <>
      {target.extras.includes("tier") && target.tiers.length > 0 && (
        <Facet
          label="Limit"
          group={`tier-${target.id}`}
          value={choices.tiers[target.id] ?? target.tiers[0]?.id ?? null}
          onChange={(id) => setTier(target.id, id)}
          disabled={disabled}
          detailInValue
          options={target.tiers.map((tier) => ({ id: tier.id, ...splitTier(tier.label) }))}
        />
      )}
      {target.extras.includes("music_format") && (
        <Facet
          label="Format"
          group="music"
          value={choices.music_format}
          onChange={setMusicFormat}
          disabled={disabled}
          options={[
            { id: "mp3", label: "MP3" },
            { id: "m4a", label: "M4A", detail: "AAC" },
          ]}
        />
      )}
    </>
  );
}

const sizeOf = (option: Pick<QualityOption, "estimated_size" | "size_range">) =>
  option.size_range
    ? formatBytesRange(option.size_range[0], option.size_range[1])
    : option.estimated_size !== null
      ? formatBytes(option.estimated_size)
      : undefined;

/**
 * "Best", then the resolutions the video really has (or bitrates for Music).
 * The size of the one pointed at shows at the end of the unfolded facet.
 */
function QualityFacet({
  registry,
  target,
  choices,
  summary,
  playlist,
  disabled,
}: {
  registry: Registry;
  target: Target;
  choices: Choices;
  summary: PlanSummary | undefined;
  playlist: boolean;
  disabled: boolean;
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
  // Best's size is known while Best is the choice: it is the plan's.
  const bestSize = selected === null && summary ? sizeOf(summary) : undefined;
  return (
    <Facet<number | "best">
      label="Quality"
      group="quality"
      value={selected ?? "best"}
      onChange={(value) => useChoices.getState().setQuality(value === "best" ? null : value, bitrate)}
      disabled={disabled}
      options={[
        { id: "best", label: "Best", detail: bestSize ?? (options.some(sizeOf) ? "Highest" : undefined) },
        ...options.map((option) => ({
          id: option.value,
          text: bitrate ? `${option.value} kbps` : `${option.value}p`,
          label: (
            <>
              {bitrate ? `${option.value} kbps` : `${option.value}p`}
              {option.converts && mixed && (
                <Hint label="Converted for this target (slower)">
                  <RefreshCw className="opacity-60" />
                </Hint>
              )}
            </>
          ),
          detail: sizeOf(option),
        })),
      ]}
    />
  );
}

/** The user's own saved presets, when there are any. */
function SavedFacet({ choices, disabled }: { choices: Choices; disabled: boolean }) {
  if (choices.saved.length === 0) return null;
  return (
    <Facet
      label="My presets"
      group="saved"
      value={choices.active.kind === "saved" ? choices.active.id : null}
      onChange={(id) => useChoices.getState().chooseSaved(id)}
      disabled={disabled}
      options={choices.saved.map((preset) => ({ id: preset.id, label: preset.name }))}
    />
  );
}
