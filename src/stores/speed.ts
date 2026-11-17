import { create } from "zustand";
import type { Speed } from "@/types";

interface SpeedState {
  /** All downloads together, in bytes per second; 0 when nothing downloads. */
  total: number;
  apply: (speed: Speed) => void;
}

export const useSpeed = create<SpeedState>((set) => ({
  total: 0,
  apply: (speed) => set({ total: speed.bytes_per_second }),
}));
