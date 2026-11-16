import { useEffect, useState } from "react";
import { formatTimecode, parseTimecode } from "@/lib/time";
import { cn } from "@/lib/utils";

/** An editable timecode. Commits on Enter or blur; ↑/↓ nudge by one frame (Shift: one second). */
export function TimeField({
  label,
  value,
  frame,
  onCommit,
  disabled,
}: {
  label: string;
  /** Seconds. */
  value: number;
  /** Length of one frame in seconds. */
  frame: number;
  onCommit: (seconds: number) => void;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  // An outside change (dragging a handle) replaces whatever was being typed.
  useEffect(() => setDraft(null), [value]);

  const commit = () => {
    if (draft === null) return;
    const seconds = parseTimecode(draft);
    setInvalid(seconds === null);
    if (seconds !== null) {
      onCommit(seconds);
      setDraft(null);
    }
  };

  return (
    <input
      aria-label={label}
      aria-invalid={invalid}
      disabled={disabled}
      value={draft ?? formatTimecode(value)}
      spellCheck={false}
      onChange={(event) => {
        setDraft(event.target.value);
        setInvalid(false);
      }}
      onFocus={(event) => event.target.select()}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          // Blurring commits (see onBlur); committing here too would do it twice.
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          setDraft(null);
          setInvalid(false);
          event.currentTarget.blur();
        } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
          event.preventDefault();
          const step = (event.shiftKey ? 1 : frame) * (event.key === "ArrowUp" ? 1 : -1);
          onCommit(value + step);
        }
      }}
      className={cn(
        "h-7 w-21 rounded-md border border-transparent bg-secondary px-2 text-center font-mono text-xs tabular-nums outline-none transition-[border-color,box-shadow] duration-150",
        "hover:border-border-strong focus:border-primary/60 focus:shadow-[0_0_0_3px_var(--primary-soft)] disabled:opacity-50",
        invalid && "border-destructive/60 focus:border-destructive/60",
      )}
    />
  );
}
