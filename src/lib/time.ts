/** 62.349 → "1:02.35"; 3725.5 → "1:02:05.50". Hundredths are enough to tell frames apart. */
export function formatTimecode(totalSeconds: number): string {
  const hundredths = Math.max(0, Math.round(totalSeconds * 100));
  const h = Math.floor(hundredths / 360000);
  const m = Math.floor((hundredths % 360000) / 6000);
  const s = Math.floor((hundredths % 6000) / 100);
  const f = hundredths % 100;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}.${pad(f)}` : `${m}:${pad(s)}.${pad(f)}`;
}

/** Like `formatTimecode`, but a whole second has no hundredths: "1:02", "1:02.35". */
export function formatShortTimecode(totalSeconds: number): string {
  const full = formatTimecode(totalSeconds);
  return full.endsWith(".00") ? full.slice(0, -3) : full;
}

/** Reads "1:02.35", "1:02:05", "62.35" or "62". Returns null for anything else. */
export function parseTimecode(text: string): number | null {
  const parts = text.trim().replace(",", ".").split(":");
  if (parts.length === 0 || parts.length > 3) return null;
  let seconds = 0;
  for (const part of parts) {
    if (!/^\d+(\.\d+)?$/.test(part)) return null;
    seconds = seconds * 60 + Number(part);
  }
  return seconds;
}

/** A short spoken length: "15.5 s", "2 min 5 s". */
export function formatLength(seconds: number): string {
  if (seconds < 60) return `${Number(seconds.toFixed(1))} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return rest === 0 ? `${minutes} min` : `${minutes} min ${rest} s`;
}
