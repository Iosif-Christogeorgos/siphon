import { useId } from "react";

import { cn } from "@/lib/utils";

/** A drop holding a play button that points down, the same shape as `app-icon.svg`. */
const DROP =
  "M244.4 56.3Q256 40 267.6 56.3L386.4 223.3A160 160 0 1 1 125.6 223.3Z" +
  "M190 277H322A15 15 0 0 1 334.66 300.04L268.66 404.04A15 15 0 0 1 243.34 404.04L177.34 300.04A15 15 0 0 1 190 277Z";

/** The mark on its own, drawn in the current text colour. */
export function LogoGlyph({ className }: { className?: string }) {
  return (
    <svg viewBox="88 36 336 448" className={className} aria-hidden="true">
      <path d={DROP} fill="currentColor" fillRule="evenodd" />
    </svg>
  );
}

/** The app icon: the mark in its aqua gradient. */
export function LogoMark({ className }: { className?: string }) {
  const gradient = useId();
  return (
    <svg
      viewBox="88 36 336 448"
      className={cn("drop-shadow-[0_6px_14px_rgb(10_143_156/0.35)]", className)}
      aria-hidden="true"
    >
      <defs>
        <linearGradient id={gradient} x1="120" y1="60" x2="400" y2="480" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#5ee8de" />
          <stop offset="1" stopColor="#07808d" />
        </linearGradient>
      </defs>
      <path d={DROP} fill={`url(#${gradient})`} fillRule="evenodd" />
    </svg>
  );
}
