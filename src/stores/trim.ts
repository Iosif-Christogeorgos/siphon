import { create } from "zustand";

export interface Segment {
  id: string;
  /** Seconds from the start of the video. */
  start: number;
  end: number;
}

/** The shortest part that can be selected, in seconds. */
export const MIN_SEGMENT = 0.1;

interface TrimState {
  open: boolean;
  segments: Segment[];
  /** The segment the handles, shortcuts and plan summary act on. */
  activeId: string | null;
  /** Opens the trimmer, starting with the whole video selected. */
  openFor: (duration: number) => void;
  close: () => void;
  /** Forgets everything; called when another video is loaded. */
  reset: () => void;
  setActive: (id: string) => void;
  /** Moves one or both edges of a segment, keeping it inside the video and non-empty. */
  setRange: (id: string, range: Partial<Pick<Segment, "start" | "end">>, duration: number) => void;
  /** Adds a part starting at the playhead. */
  addSegment: (at: number, duration: number) => void;
  removeSegment: (id: string) => void;
}

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

export const useTrim = create<TrimState>((set, get) => ({
  open: false,
  segments: [],
  activeId: null,

  openFor: (duration) => {
    if (get().segments.length > 0) return set({ open: true });
    const id = crypto.randomUUID();
    set({ open: true, segments: [{ id, start: 0, end: duration }], activeId: id });
  },
  close: () => set({ open: false }),
  reset: () => set({ open: false, segments: [], activeId: null }),
  setActive: (activeId) => set({ activeId }),

  setRange: (id, range, duration) =>
    set({
      activeId: id,
      segments: get().segments.map((segment) => {
        if (segment.id !== id) return segment;
        // The edge being moved yields to the other one, never pushes it.
        const start =
          range.start === undefined
            ? segment.start
            : clamp(range.start, 0, (range.end ?? segment.end) - MIN_SEGMENT);
        const end =
          range.end === undefined ? segment.end : clamp(range.end, start + MIN_SEGMENT, duration);
        return { ...segment, start, end };
      }),
    }),

  addSegment: (at, duration) => {
    const start = clamp(at, 0, duration - MIN_SEGMENT);
    // A tenth of the video (at least five seconds) is a comfortable handle to grab.
    const end = Math.min(duration, start + Math.max(5, duration * 0.1));
    const id = crypto.randomUUID();
    set({ segments: [...get().segments, { id, start, end }], activeId: id });
  },

  removeSegment: (id) => {
    const segments = get().segments.filter((segment) => segment.id !== id);
    const activeId = get().activeId === id ? (segments[0]?.id ?? null) : get().activeId;
    set({ segments, activeId });
  },
}));
