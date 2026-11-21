import {
  Apple,
  AudioLines,
  Gem,
  Headphones,
  type LucideIcon,
  MessageCircle,
  Monitor,
  Music,
  Phone,
  Play,
  Send,
  Smartphone,
  Sparkles,
  Tv,
} from "lucide-react";

/** The icons the preset registry names (see `icon` in plan/registry.rs). */
const ICONS: Record<string, LucideIcon> = {
  apple: Apple,
  "audio-lines": AudioLines,
  gem: Gem,
  headphones: Headphones,
  "message-circle": MessageCircle,
  monitor: Monitor,
  music: Music,
  phone: Phone,
  play: Play,
  send: Send,
  smartphone: Smartphone,
  sparkles: Sparkles,
  tv: Tv,
};

export const iconFor = (name: string): LucideIcon => ICONS[name] ?? Play;
