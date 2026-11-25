import { Check, ImageOff } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** Lower case, without accents, for matching names: "Café" is found by "cafe". */
export const fold = (text: string) =>
  text
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();

/** The square tick box a list's rows and its options share. */
export function Box({ checked, disabled = false }: { checked: boolean; disabled?: boolean }) {
  return (
    <span
      className={cn(
        "grid size-4.5 shrink-0 place-items-center rounded-md border transition-colors duration-150",
        checked ? "border-primary bg-primary text-primary-foreground" : "border-border-strong",
        disabled && "opacity-40",
      )}
    >
      {checked && <Check className="size-3" strokeWidth={3} />}
    </span>
  );
}

/** A row's small picture, or what stands in for a missing one. */
export function Thumb({ src, empty }: { src: string | null; empty?: ReactNode }) {
  return (
    <span className="relative aspect-video w-14 shrink-0 overflow-hidden rounded-md bg-muted">
      {src ? (
        <img src={src} alt="" loading="lazy" draggable={false} referrerPolicy="no-referrer" className="size-full object-cover" />
      ) : (
        <span className="grid size-full place-items-center text-subtle-foreground">
          {empty ?? <ImageOff className="size-3" />}
        </span>
      )}
    </span>
  );
}
