const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** 1536 → "1.5 KB". Binary units, matching what Windows Explorer shows. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "—";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${BYTE_UNITS[unit]}`;
}

/** A size that may land anywhere between two values, in one unit: "1.4–5.6 GB", "52–126 MB". */
export function formatBytesRange(low: number, high: number): string {
  let unit = 0;
  let scale = 1;
  while (high / scale >= 1024 && unit < BYTE_UNITS.length - 1) {
    scale *= 1024;
    unit += 1;
  }
  const digits = unit > 0 && high / scale < 10 ? 1 : 0;
  const value = (bytes: number) => (bytes / scale).toFixed(digits);
  return `${value(low)}–${value(high)} ${BYTE_UNITS[unit]}`;
}

export function formatSpeed(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond == null || bytesPerSecond <= 0) return "";
  return `${formatBytes(bytesPerSecond)}/s`;
}

/** 3725 → "1:02:05", 65 → "1:05". */
export function formatDuration(totalSeconds: number | null | undefined): string {
  if (totalSeconds == null || !Number.isFinite(totalSeconds)) return "";
  const seconds = Math.max(0, Math.round(totalSeconds));
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** Time remaining in plain words: 42 → "42s left", 190 → "3 min left". */
export function formatEta(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "";
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))}s left`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min left`;
  const h = Math.floor(seconds / 3600);
  const m = Math.round((seconds % 3600) / 60);
  return `${h}h ${m}m left`;
}

/** 1234567 → "1.2M". */
export function formatCount(value: number | null | undefined): string {
  if (value == null) return "";
  return new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 }).format(
    value,
  );
}
