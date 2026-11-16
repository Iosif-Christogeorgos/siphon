import {
  AppWindow,
  AudioLines,
  Bot,
  Clapperboard,
  Gem,
  Laptop,
  type LucideIcon,
  Send,
  SlidersHorizontal,
  Smartphone,
  Tv,
} from "lucide-react";
import { Chip } from "@/components/ui/chip";
import { useOptions } from "@/stores/options";
import type { PresetId } from "@/types";

const PRESETS: { id: PresetId; label: string; icon: LucideIcon }[] = [
  { id: "iphone", label: "iPhone", icon: Smartphone },
  { id: "android", label: "Android", icon: Bot },
  { id: "windows", label: "Windows", icon: AppWindow },
  { id: "macos", label: "Mac", icon: Laptop },
  { id: "tv", label: "Smart TV", icon: Tv },
  { id: "editing", label: "Editing", icon: Clapperboard },
  { id: "share", label: "Discord / WhatsApp", icon: Send },
  { id: "audio", label: "Audio only", icon: AudioLines },
  { id: "max", label: "Max quality", icon: Gem },
];

/** "Where will you watch it?" One chip per preset, plus Custom for manual mode. */
export function PresetPicker({ disabled }: { disabled: boolean }) {
  const mode = useOptions((state) => state.mode);
  const preset = useOptions((state) => state.preset);
  const choosePreset = useOptions((state) => state.choosePreset);
  const chooseManual = useOptions((state) => state.chooseManual);

  return (
    <div role="radiogroup" aria-label="Preset" className="flex flex-wrap gap-1">
      {PRESETS.map(({ id, label, icon: Icon }) => (
        <Chip
          key={id}
          group="preset"
          selected={mode === "preset" && preset === id}
          onSelect={() => choosePreset(id)}
          disabled={disabled}
        >
          <Icon />
          {label}
        </Chip>
      ))}
      <Chip group="preset" selected={mode === "manual"} onSelect={chooseManual} disabled={disabled}>
        <SlidersHorizontal />
        Custom
      </Chip>
    </div>
  );
}
