import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { create } from "zustand";
import { ipc } from "@/lib/ipc";
import type {
  Choice,
  Choices,
  GroupId,
  ManualOptions,
  MusicFormat,
  Placement,
  Registry,
  Target,
  TargetId,
  TierId,
} from "@/types";

/** The webview storage an older Siphon kept its choice in. */
const LEGACY_KEY = "siphon.options";

/** Saving waits this long, so a burst of clicks is written once. */
const SAVE_DELAY_MS = 300;

interface ChoicesState {
  /** null until loaded from the backend. */
  choices: Choices | null;
  /** Loads what was chosen last time, taking over an older version's choice once. */
  load: () => Promise<void>;
  chooseGroup: (group: GroupId) => void;
  chooseTarget: (group: GroupId, target: TargetId) => void;
  /** A resolution, or a bitrate for Music (`bitrate`); null is "Best". */
  setQuality: (value: number | null, bitrate: boolean) => void;
  setTier: (target: TargetId, tier: TierId) => void;
  setMusicFormat: (format: MusicFormat) => void;
  /** The language subtitles are wanted in, for every video that has it; null is none. */
  setSubtitles: (language: string | null) => void;
  /** Draw a Twitch video's chat into the picture, or not. */
  setChat: (on: boolean) => void;
  /** Where the chat goes for a channel's videos. */
  placeChat: (channel: string, placement: Placement) => void;
  /** Opens the custom settings, starting from the ones last used. */
  customize: () => void;
  setCustom: (changes: Partial<ManualOptions>) => void;
  /** Saves the custom settings as a named preset and selects it. */
  savePreset: (name: string) => void;
  chooseSaved: (id: string) => void;
  deleteSaved: (id: string) => void;
  /** Forgets the stream picks, which belong to one video. */
  resetForVideo: () => void;
}

let saveTimer: number | undefined;

const AUDIO_CONTAINERS = new Set(["m4a", "mp3", "opus", "ogg", "flac", "wav"]);

export const useChoices = create<ChoicesState>((set, get) => {
  /** Applies a change now and saves it shortly after. */
  const change = (update: (choices: Choices) => Partial<Choices>) => {
    const current = get().choices;
    if (!current) return;
    const next = { ...current, ...update(current) };
    set({ choices: next });
    window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => void ipc.setChoices(next).catch(() => {}), SAVE_DELAY_MS);
  };

  return {
    choices: null,

    load: async () => {
      let legacy: unknown = null;
      try {
        const stored = localStorage.getItem(LEGACY_KEY);
        legacy = stored ? (JSON.parse(stored) as { state?: unknown }).state : null;
      } catch {
        // Unreadable storage: nothing to take over.
      }
      const choices = legacy ? await ipc.importLegacyChoices(legacy) : await ipc.getChoices();
      if (legacy) {
        try {
          localStorage.removeItem(LEGACY_KEY);
        } catch {
          // It is only read once anyway: the backend ignores it after this.
        }
      }
      set({ choices });
    },

    chooseGroup: (group) => change(() => ({ group, active: { kind: "preset" } })),
    chooseTarget: (group, target) =>
      change((choices) => ({ group, targets: { ...choices.targets, [group]: target }, active: { kind: "preset" } })),
    setQuality: (value, bitrate) => change(() => (bitrate ? { music_kbps: value } : { quality: value })),
    setTier: (target, tier) => change((choices) => ({ tiers: { ...choices.tiers, [target]: tier } })),
    setMusicFormat: (music_format) => change(() => ({ music_format })),
    setSubtitles: (subtitles) => change(() => ({ subtitles })),
    setChat: (chat) => change(() => ({ chat })),
    placeChat: (channel, placement) =>
      change((choices) => ({ chat_places: { ...choices.chat_places, [channel]: placement } })),

    customize: () =>
      change((choices) => {
        // Editing a saved preset starts from its settings.
        const saved = choices.active.kind === "saved" ? savedOf(choices) : undefined;
        return { active: { kind: "custom" }, custom: saved ? { ...saved.options } : choices.custom };
      }),
    setCustom: (changes) =>
      change((choices) => {
        // Changing a saved preset's settings starts from them (it isn't overwritten).
        const custom = { ...(savedOf(choices)?.options ?? choices.custom), ...changes };
        // A container chosen for video makes no sense for audio only, and vice versa.
        const audioOnly = custom.content === "audio_only";
        if (custom.container && AUDIO_CONTAINERS.has(custom.container) !== audioOnly) custom.container = null;
        return { custom, active: { kind: "custom" } };
      }),
    savePreset: (name) =>
      change((choices) => {
        const id = crypto.randomUUID();
        const options = { ...choices.custom, video_format: null, audio_format: null };
        return { saved: [...choices.saved, { id, name: name.trim(), options }], active: { kind: "saved", id } };
      }),
    chooseSaved: (id) => change(() => ({ active: { kind: "saved", id } })),
    deleteSaved: (id) =>
      change((choices) => ({
        saved: choices.saved.filter((preset) => preset.id !== id),
        active: choices.active.kind === "saved" && choices.active.id === id ? { kind: "custom" } : choices.active,
      })),

    resetForVideo: () => {
      const current = get().choices;
      if (current?.custom.video_format || current?.custom.audio_format) {
        set({ choices: { ...current, custom: { ...current.custom, video_format: null, audio_format: null } } });
      }
    },
  };
});

function savedOf(choices: Choices) {
  return choices.active.kind === "saved"
    ? choices.saved.find((preset) => preset.id === (choices.active as { id: string }).id)
    : undefined;
}

/** Every preset group, target and tier, from the backend. */
export function useRegistry() {
  return useQuery({ queryKey: ["registry"], queryFn: ipc.getPresets, staleTime: Number.POSITIVE_INFINITY }).data;
}

/** The target picked in the current group. */
export function currentTarget(registry: Registry, choices: Choices): Target | undefined {
  const group = registry.groups.find((candidate) => candidate.id === choices.group) ?? registry.groups[0];
  if (!group) return undefined;
  const id = choices.targets[group.id] ?? group.default_target;
  return group.targets.find((target) => target.id === id) ?? group.targets[0];
}

/** Down the right edge, mostly clear: mirrors `Placement::DEFAULT` in chat/mod.rs. */
export const DEFAULT_PLACEMENT: Placement = { x: 0.765, y: 0.3, width: 0.22, height: 0.64, shade: 0.3 };

/** Where the chat goes for `channel`'s videos: where it was placed last, or the default. */
export function placementFor(choices: Choices, channel: string): Placement {
  return choices.chat_places[channel] ?? DEFAULT_PLACEMENT;
}

/**
 * The choice as the backend plans it. `chatChannel` is the channel of a video
 * whose chat can be drawn in: with chat on, its placement goes with the choice.
 */
export function toChoice(registry: Registry, choices: Choices, chatChannel: string | null = null): Choice | null {
  if (choices.active.kind !== "preset") {
    const options = savedOf(choices)?.options ?? choices.custom;
    return { mode: "custom", ...options, subtitles: choices.subtitles };
  }
  const target = currentTarget(registry, choices);
  if (!target) return null;
  return {
    mode: "preset",
    target: target.id,
    quality: target.quality === "bitrate" ? choices.music_kbps : choices.quality,
    tier: choices.tiers[target.id] ?? null,
    music_format: choices.music_format,
    subtitles: choices.subtitles,
    chat: choices.chat && chatChannel !== null ? placementFor(choices, chatChannel) : null,
  };
}

/** The current choice, or null while the registry or the choices load; see [`toChoice`] for `chatChannel`. */
export function useChoice(chatChannel: string | null = null): Choice | null {
  const registry = useRegistry();
  const choices = useChoices((state) => state.choices);
  return useMemo(
    () => (registry && choices ? toChoice(registry, choices, chatChannel) : null),
    [registry, choices, chatChannel],
  );
}
