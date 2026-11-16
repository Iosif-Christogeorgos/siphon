import { cn } from "@/lib/utils";

/** The siphon glyph on its own, drawn in the current text colour. */
export function LogoGlyph({ className }: { className?: string }) {
  return (
    <svg viewBox="96 96 320 368" className={className} aria-hidden="true">
      <path
        d="M176 262V214a80 80 0 0 1 160 0v104"
        fill="none"
        stroke="currentColor"
        strokeWidth="54"
        strokeLinecap="round"
      />
      <path
        d="M336 356c24 30 37 48 37 67a37 37 0 0 1-74 0c0-19 13-37 37-67z"
        fill="currentColor"
      />
    </svg>
  );
}

/** The app icon: the glyph on its aqua tile. */
export function LogoTile({ className }: { className?: string }) {
  return (
    <div
      className={cn(
        "grid place-items-center rounded-[28%] bg-linear-to-br from-[#3fd8cf] to-[#0a8f9c] text-white shadow-[inset_0_1px_0_rgb(255_255_255/0.35),0_8px_24px_-6px_rgb(10_143_156/0.55)]",
        className,
      )}
    >
      <LogoGlyph className="size-[58%]" />
    </div>
  );
}
