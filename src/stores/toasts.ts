import { create } from "zustand";

export interface Toast {
  id: string;
  title: string;
  /** One line of detail under the title. */
  description?: string;
  /** A single thing the user can do about it. */
  action?: { label: string; run: () => void };
}

interface ToastsState {
  toasts: Toast[];
  /** Shows a toast. One with the same `id` replaces the earlier one instead of stacking. */
  show: (toast: Omit<Toast, "id"> & { id?: string }) => void;
  dismiss: (id: string) => void;
}

/** More than this at once is noise; the oldest make room. */
const MAX_VISIBLE = 3;

export const useToasts = create<ToastsState>((set, get) => ({
  toasts: [],
  show: (toast) => {
    const id = toast.id ?? crypto.randomUUID();
    const others = get().toasts.filter((existing) => existing.id !== id);
    set({ toasts: [...others, { ...toast, id }].slice(-MAX_VISIBLE) });
  },
  dismiss: (id) => set({ toasts: get().toasts.filter((toast) => toast.id !== id) }),
}));
