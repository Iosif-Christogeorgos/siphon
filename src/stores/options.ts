import { useMemo } from "react";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { DownloadOptions, ManualOptions, PresetId } from "@/types";

export type ManualFields = Omit<ManualOptions, "mode">;

/** Upload limits offered by the Share preset (mirrors `SHARE_LIMITS_MB` in presets.rs). */
export const SHARE_LIMITS_MB = [10, 16, 50, 100, 500] as const;

const AUDIO_CONTAINERS = new Set(["m4a", "mp3", "opus", "ogg", "flac", "wav"]);

interface OptionsState {
  mode: "preset" | "manual";
  preset: PresetId;
  /** Resolution picked for this video; null means "let the preset decide". */
  maxLines: number | null;
  sizeLimitMb: number;
  manual: ManualFields;
  choosePreset: (preset: PresetId) => void;
  chooseManual: () => void;
  setMaxLines: (lines: number | null) => void;
  setSizeLimit: (megabytes: number) => void;
  setManual: (changes: Partial<ManualFields>) => void;
  /** Forgets the choices that only make sense for one video. */
  resetForVideo: () => void;
}

export const useOptions = create<OptionsState>()(
  persist(
    (set, get) => ({
      mode: "preset",
      preset: "windows",
      maxLines: null,
      sizeLimitMb: SHARE_LIMITS_MB[0],
      manual: {
        content: "video_audio",
        video_format: null,
        audio_format: null,
        container: null,
        video_codec: null,
        audio_codec: null,
      },

      choosePreset: (preset) => set({ mode: "preset", preset, maxLines: null }),
      chooseManual: () => set({ mode: "manual" }),
      setMaxLines: (maxLines) => set({ maxLines }),
      setSizeLimit: (sizeLimitMb) => set({ sizeLimitMb }),

      setManual: (changes) => {
        const manual = { ...get().manual, ...changes };
        // A container chosen for video makes no sense for audio-only, and vice versa.
        const audioOnly = manual.content === "audio_only";
        if (manual.container && AUDIO_CONTAINERS.has(manual.container) !== audioOnly) {
          manual.container = null;
        }
        set({ manual });
      },

      resetForVideo: () =>
        set({ maxLines: null, manual: { ...get().manual, video_format: null, audio_format: null } }),
    }),
    {
      name: "siphon.options",
      // Stream ids and the resolution belong to one video; everything else is a preference.
      partialize: ({ mode, preset, sizeLimitMb, manual }) => ({
        mode,
        preset,
        sizeLimitMb,
        manual: { ...manual, video_format: null, audio_format: null },
      }),
    },
  ),
);

/** The current choices in the shape the backend expects. */
export function useDownloadOptions(): DownloadOptions {
  const mode = useOptions((state) => state.mode);
  const preset = useOptions((state) => state.preset);
  const maxLines = useOptions((state) => state.maxLines);
  const sizeLimitMb = useOptions((state) => state.sizeLimitMb);
  const manual = useOptions((state) => state.manual);

  return useMemo(
    () =>
      mode === "manual"
        ? { mode: "manual", ...manual }
        : {
            mode: "preset",
            preset,
            max_lines: maxLines,
            size_limit_mb: preset === "share" ? sizeLimitMb : null,
          },
    [mode, preset, maxLines, sizeLimitMb, manual],
  );
}
