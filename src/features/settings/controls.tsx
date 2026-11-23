import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Check, FileVideo, FolderOpen, RotateCcw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type KeyboardEvent, type ReactNode, useEffect, useState } from "react";
import { onRadioKeys } from "@/components/ui/radio-keys";
import { Segmented } from "@/components/ui/segmented";
import { Hint } from "@/components/ui/tooltip";
import { MB_PER_SECOND } from "@/lib/format";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import type { Theme } from "@/stores/theme";
import type { FilenameStyle } from "@/types";
import { fieldClass, Reveal } from "./parts";

// ── Save to ─────────────────────────────────────────────────────────────

/**
 * Where downloads go, as a folder: its name large, where it is underneath
 * (cut from the left, so the end of the path stays in view). The whole tile
 * opens the folder picker.
 */
export function FolderTile({
  folder,
  custom,
  onChange,
}: {
  /** The folder in use. */
  folder: string;
  /** Whether the user chose it, rather than the system's Downloads folder. */
  custom: boolean;
  onChange: (folder: string | null) => void;
}) {
  const choose = async () => {
    const picked = await openDialog({ directory: true, defaultPath: folder });
    if (typeof picked === "string") onChange(picked);
  };
  const parts = folder.split(/[\\/]+/).filter(Boolean);
  const name = parts.at(-1) ?? folder;
  const parent = parts.slice(0, -1).join("\\");

  return (
    <div className="group/folder flex items-center gap-1 rounded-xl border border-border-strong bg-background/60 p-1.5 transition-[border-color,background-color] duration-200 hover:border-primary/40 hover:bg-primary-soft/40">
      <div
        // A large target for the pointer; Change is the control for the keyboard and screen readers.
        onClick={() => void choose()}
        className="flex min-w-0 flex-1 cursor-pointer items-center gap-3 pl-1"
      >
        <div className="grid size-9 shrink-0 place-items-center rounded-lg bg-primary-soft text-primary ring-1 ring-primary/15 transition-transform duration-300 ease-out-expo group-hover/folder:scale-105">
          <FolderOpen className="size-4" strokeWidth={1.75} />
        </div>
        <div className="min-w-0 flex-1 leading-tight">
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.div
              key={folder}
              initial={{ opacity: 0, y: 6, filter: "blur(3px)" }}
              animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              exit={{ opacity: 0, y: -6, filter: "blur(3px)" }}
              transition={spring}
            >
              <p title={folder} className="truncate text-[13px] font-medium">
                {name}
              </p>
              {parent && (
                // Right to left, so the ellipsis falls at the start; <bdi> keeps the path itself left to right.
                <p dir="rtl" title={folder} className="truncate text-left text-[11px] text-subtle-foreground">
                  <bdi dir="ltr">{parent}</bdi>
                </p>
              )}
            </motion.div>
          </AnimatePresence>
        </div>
      </div>
      <AnimatePresence initial={false}>
        {custom && (
          <motion.div
            key="reset"
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.6 }}
            transition={spring}
          >
            <Hint label="Use the Downloads folder again">
              <button
                type="button"
                aria-label="Use the Downloads folder again"
                onClick={() => onChange(null)}
                className="grid size-8 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              >
                <RotateCcw className="size-3.5" />
              </button>
            </Hint>
          </motion.div>
        )}
      </AnimatePresence>
      <button
        type="button"
        onClick={() => void choose()}
        className="h-8 rounded-lg bg-secondary px-3 text-[13px] font-medium transition-[background-color,transform] duration-150 hover:bg-border-strong active:scale-[0.97]"
      >
        Change
      </button>
    </div>
  );
}

// ── File names ──────────────────────────────────────────────────────────

const FILENAME_STYLES: { id: FilenameStyle; label: string; hint: string; template: string }[] = [
  { id: "title_id", label: "Title [id]", hint: "The title and the video’s id", template: "%(title)s [%(id)s]" },
  { id: "title", label: "Title", hint: "The title alone", template: "%(title)s" },
  { id: "channel_title", label: "Channel", hint: "The channel, then the title", template: "%(channel)s - %(title)s" },
  { id: "date_title", label: "Date", hint: "The upload date, then the title", template: "%(upload_date>%Y-%m-%d)s %(title)s" },
  { id: "custom", label: "Custom", hint: "Your own yt-dlp template", template: "" },
];

/** The video the examples are of. */
const SAMPLE: Record<string, string> = {
  title: "Big Buck Bunny",
  fulltitle: "Big Buck Bunny",
  id: "aqz-KE-bpKQ",
  display_id: "aqz-KE-bpKQ",
  channel: "Blender",
  uploader: "Blender",
  creator: "Blender",
  channel_id: "UCSMOQeBJ2RAnuFungnQOxLg",
  upload_date: "20141110",
  release_date: "20141110",
  extractor: "youtube",
  extractor_key: "Youtube",
  webpage_url_domain: "youtube.com",
  duration: "634",
  duration_string: "10:34",
  width: "3840",
  height: "2160",
  resolution: "3840x2160",
  fps: "60",
  vcodec: "vp09",
  acodec: "opus",
  format_id: "315+251",
  ext: "mp4",
};

/**
 * What a yt-dlp output template makes of the sample video, near enough for a
 * preview: fields, alternatives (`a,b`), defaults (`|x`) and date formats
 * (`>%Y-%m-%d`). A field the sample doesn't have comes out as yt-dlp writes
 * it, "NA".
 */
function renderTemplate(template: string): string {
  return template.replace(/%%|%\(([^)]*)\)[-#0+ ]*\d*(?:\.\d+)?[a-zA-Z]/g, (_match, inner: string | undefined) => {
    if (inner === undefined) return "%";
    const [expression = "", fallback] = inner.split("|");
    for (const alternative of expression.split(",")) {
      const [field = "", format] = alternative.split(">");
      const value = SAMPLE[field.trim()];
      if (value === undefined) continue;
      if (format && /^\d{8}$/.test(value)) {
        const parts: Record<string, string> = { Y: value.slice(0, 4), y: value.slice(2, 4), m: value.slice(4, 6), d: value.slice(6, 8) };
        return format.replace(/%([Yymd])/g, (_, key: string) => parts[key] ?? "");
      }
      return value;
    }
    return fallback ?? "NA";
  });
}

/**
 * How downloads are named: a style, and the name it gives the sample video,
 * changing as the style does. Custom opens a template field whose example
 * follows the typing.
 */
export function FileNames({
  style,
  template,
  onStyle,
  onTemplate,
  error,
}: {
  style: FilenameStyle;
  template: string;
  onStyle: (style: FilenameStyle) => void;
  onTemplate: (template: string) => void;
  error: string | null;
}) {
  // Saved when the field loses focus, not on every keystroke; the example follows every keystroke.
  const [draft, setDraft] = useState(template);
  useEffect(() => setDraft(template), [template]);
  const custom = style === "custom";
  const chosen = FILENAME_STYLES.find((candidate) => candidate.id === style);
  const example = renderTemplate(custom ? draft : (chosen?.template ?? "")).trim();

  return (
    <div className="space-y-2">
      <Segmented
        label="File names"
        group="filename"
        className="w-full"
        value={style}
        options={FILENAME_STYLES.map(({ id, label, hint }) => ({ id, label, hint }))}
        onChange={onStyle}
      />
      <Reveal open={custom}>
        <div className="pt-0.5">
          <input
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => draft !== template && onTemplate(draft)}
            onKeyDown={(event) => event.key === "Enter" && event.currentTarget.blur()}
            spellCheck={false}
            placeholder="%(channel)s - %(title)s"
            aria-label="Custom file name template"
            className={cn(fieldClass, "w-full font-mono text-xs")}
          />
        </div>
      </Reveal>
      <FilePreview id={style} stem={example || "…"} />
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      <Reveal open={custom}>
        <p className="pb-0.5 text-xs text-pretty text-muted-foreground">
          yt-dlp’s fields, such as <Code>%(title)s</Code>, <Code>%(channel)s</Code>, <Code>%(id)s</Code> and{" "}
          <Code>%(upload_date)s</Code>. The extension is added for you.
        </p>
      </Reveal>
    </div>
  );
}

/** A file as it will be named: the name, then its extension, a little fainter. */
function FilePreview({ id, stem }: { id: string; stem: string }) {
  return (
    <div className="flex min-h-9 items-center gap-2.5 rounded-lg border border-dashed border-border-strong px-3 py-2">
      <FileVideo className="size-3.5 shrink-0 text-subtle-foreground" />
      <div className="relative min-w-0 flex-1">
        {/* A new style slides its example in; typing a template only changes the text. */}
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.p
            key={id}
            data-selectable
            title={`${stem}.mp4`}
            className="line-clamp-2 font-mono text-[11.5px] leading-snug wrap-break-word text-foreground/90"
            initial={{ opacity: 0, x: 10, filter: "blur(3px)" }}
            animate={{ opacity: 1, x: 0, filter: "blur(0px)" }}
            exit={{ opacity: 0, x: -10, filter: "blur(3px)" }}
            transition={spring}
          >
            {stem}
            <span className="text-subtle-foreground">.mp4</span>
          </motion.p>
        </AnimatePresence>
      </div>
    </div>
  );
}

function Code({ children }: { children: ReactNode }) {
  return <code className="rounded bg-secondary px-1 py-0.5 font-mono text-[11px]">{children}</code>;
}

// ── At the same time ────────────────────────────────────────────────────

/**
 * How many downloads run at once, as a row of bars rising like a signal: as
 * many light up as will run. Click a bar, or use the arrow keys; pointing at
 * one shows what clicking would choose.
 */
export function Lanes({
  label,
  value,
  max,
  onChange,
}: {
  label: string;
  value: number;
  max: number;
  onChange: (value: number) => void;
}) {
  const [pointed, setPointed] = useState<number | null>(null);
  const [direction, setDirection] = useState(1);
  const set = (next: number) => {
    const clamped = Math.min(max, Math.max(1, next));
    if (clamped === value) return;
    setDirection(clamped > value ? 1 : -1);
    onChange(clamped);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = { ArrowLeft: value - 1, ArrowDown: value - 1, ArrowRight: value + 1, ArrowUp: value + 1, Home: 1, End: max }[
      event.key
    ];
    if (next === undefined) return;
    event.preventDefault();
    set(next);
  };
  const shown = pointed ?? value;

  return (
    <div
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={1}
      aria-valuemax={max}
      aria-valuenow={value}
      onKeyDown={onKeyDown}
      className="flex items-center gap-2.5 rounded-lg py-0.5 pr-1 pl-2 outline-none focus-visible:ring-[3px] focus-visible:ring-ring/40"
    >
      <div className="flex h-6 items-end" onPointerLeave={() => setPointed(null)}>
        {Array.from({ length: max }, (_, index) => {
          const n = index + 1;
          const lit = n <= value;
          const preview = pointed !== null && n <= pointed && !lit;
          return (
            <span
              key={n}
              onPointerEnter={() => setPointed(n)}
              onClick={() => set(n)}
              className="flex h-full cursor-pointer items-end px-[2.5px]"
            >
              <span
                className="relative w-[5px] overflow-hidden rounded-full bg-border-strong"
                style={{ height: 10 + index * 2.6 }}
              >
                <motion.span
                  className="absolute inset-0 rounded-full bg-primary"
                  initial={false}
                  animate={{ y: lit || preview ? "0%" : "101%", opacity: lit ? 1 : preview ? 0.3 : 0 }}
                  // The bars fill one after another, from the side the change came from.
                  transition={{ ...spring, delay: Math.abs(n - (direction > 0 ? 1 : max)) * 0.018 }}
                />
                {lit && <span className="absolute inset-0 rounded-full shadow-[0_0_8px_var(--primary-glow)]" />}
              </span>
            </span>
          );
        })}
      </div>
      <span className="relative grid h-6 w-4 place-items-center overflow-hidden text-[13px] font-semibold tabular-nums">
        <AnimatePresence mode="popLayout" initial={false} custom={direction}>
          <motion.span
            key={shown}
            custom={direction}
            className={cn(pointed !== null && pointed !== value && "text-muted-foreground")}
            variants={{
              enter: (dir: number) => ({ y: dir * 12, opacity: 0 }),
              center: { y: 0, opacity: 1 },
              exit: (dir: number) => ({ y: dir * -12, opacity: 0 }),
            }}
            initial="enter"
            animate="center"
            exit="exit"
            transition={spring}
          >
            {shown}
          </motion.span>
        </AnimatePresence>
      </span>
    </div>
  );
}

// ── Speed limit ─────────────────────────────────────────────────────────

/**
 * The speed limits offered with one click, in MB/s. Any other limit shows as
 * Custom, with its value: that includes one saved from an earlier set of
 * presets (2, 5, 10 and 25), so changing this list never changes a limit.
 */
const SPEED_LIMITS = [20, 30, 40, 50];

/**
 * One limit for all downloads together: off, a few common speeds, or the
 * user's own. A new limit applies to running downloads at once.
 */
export function SpeedLimit({ limit, onChange }: { limit: number | null; onChange: (limit: number | null) => void }) {
  const preset = limit === null ? "off" : SPEED_LIMITS.find((mb) => mb * MB_PER_SECOND === limit);
  // Custom stays open once chosen, even before a number is typed.
  const [customOpen, setCustomOpen] = useState(preset === undefined);
  // Focus the field when Custom is picked, not when the panel opens on a custom limit.
  const [focusField, setFocusField] = useState(false);
  const custom = customOpen || preset === undefined;
  const toText = (bytes: number | null) => (bytes === null ? "" : String(Number((bytes / MB_PER_SECOND).toFixed(2))));
  const [draft, setDraft] = useState(() => toText(limit));
  useEffect(() => setDraft(toText(limit)), [limit]);

  const choose = (id: number | "off" | "custom") => {
    setCustomOpen(id === "custom");
    setFocusField(id === "custom");
    if (id === "off") onChange(null);
    else if (typeof id === "number") onChange(id * MB_PER_SECOND);
  };
  const commit = () => {
    const mb = Number.parseFloat(draft.replace(",", "."));
    if (Number.isFinite(mb) && mb > 0) {
      const bytes = Math.round(mb * MB_PER_SECOND);
      if (bytes !== limit) onChange(bytes);
    } else {
      setDraft(toText(limit));
    }
  };

  return (
    <div>
      <Segmented<number | "off" | "custom">
        label="Speed limit"
        group="speed-limit"
        className="w-full"
        value={custom ? "custom" : (preset ?? null)}
        options={[
          { id: "off", label: "Unlimited" },
          ...SPEED_LIMITS.map((mb) => ({ id: mb, label: String(mb) })),
          { id: "custom", label: "Custom" },
        ]}
        onChange={choose}
      />
      <Reveal open={custom}>
        <div className="pt-2.5 pb-1">
          <label className="relative block w-36">
            <input
              autoFocus={focusField}
              // Typing replaces the limit shown rather than adding to it.
              onFocus={(event) => event.currentTarget.select()}
              inputMode="decimal"
              value={draft}
              placeholder="3.5"
              onChange={(event) => setDraft(event.target.value)}
              onBlur={commit}
              onKeyDown={(event) => event.key === "Enter" && event.currentTarget.blur()}
              aria-label="Custom speed limit in MB/s"
              className={cn(fieldClass, "w-full pr-14 tabular-nums")}
            />
            <span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-xs text-subtle-foreground">
              MB/s
            </span>
          </label>
        </div>
      </Reveal>
    </div>
  );
}

// ── Theme ───────────────────────────────────────────────────────────────

const THEMES: { id: Theme; label: string }[] = [
  { id: "system", label: "System" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
];

/** The theme, picked from three small pictures of Siphon in it. */
export function ThemePicker({ value, onChange }: { value: Theme; onChange: (theme: Theme) => void }) {
  return (
    <div role="radiogroup" aria-label="Theme" onKeyDown={onRadioKeys} className="grid grid-cols-3 gap-2.5">
      {THEMES.map(({ id, label }) => {
        const selected = id === value;
        return (
          <button
            key={id}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(id)}
            className="group/theme flex flex-col items-stretch gap-2 rounded-xl outline-none"
          >
            <span className="relative block rounded-xl p-[3px]">
              {selected && (
                <motion.span
                  layoutId="theme-ring"
                  transition={spring}
                  className="absolute inset-0 rounded-xl ring-[1.5px] ring-primary shadow-[0_0_18px_-4px_var(--primary-glow)]"
                />
              )}
              <span
                className={cn(
                  "relative block aspect-[16/11] overflow-hidden rounded-[9px] border transition-transform duration-300 ease-out-expo",
                  "group-hover/theme:-translate-y-0.5 group-focus-visible/theme:ring-[3px] group-focus-visible/theme:ring-ring/40",
                )}
              >
                {id === "system" ? (
                  <>
                    <Miniature scheme="dark" />
                    {/* Light on the other side of a diagonal. */}
                    <span className="absolute inset-0 [clip-path:polygon(62%_0,100%_0,100%_100%,38%_100%)]">
                      <Miniature scheme="light" />
                    </span>
                  </>
                ) : (
                  <Miniature scheme={id} />
                )}
              </span>
            </span>
            <span
              className={cn(
                "flex items-center justify-center gap-1 text-xs font-medium transition-colors duration-150",
                selected ? "text-primary" : "text-muted-foreground group-hover/theme:text-foreground",
              )}
            >
              <AnimatePresence initial={false}>
                {selected && (
                  <motion.span
                    initial={{ width: 0, opacity: 0, scale: 0.5 }}
                    animate={{ width: "auto", opacity: 1, scale: 1 }}
                    exit={{ width: 0, opacity: 0, scale: 0.5 }}
                    transition={spring}
                    className="flex"
                  >
                    <Check className="size-3" strokeWidth={2.5} />
                  </motion.span>
                )}
              </AnimatePresence>
              {label}
            </span>
          </button>
        );
      })}
    </div>
  );
}

/**
 * Siphon's start screen in miniature. It is drawn with the app's own colours,
 * under the scheme it shows (`color-scheme` decides which side of each
 * `light-dark()` colour applies), so it always matches the real thing.
 */
function Miniature({ scheme }: { scheme: "light" | "dark" }) {
  return (
    <span aria-hidden style={{ colorScheme: scheme }} className="absolute inset-0 block bg-background">
      <span className="absolute inset-x-0 top-0 h-3/4 bg-[radial-gradient(70%_100%_at_50%_0%,var(--primary-soft),transparent)]" />
      <span className="absolute top-[9%] left-[8%] flex items-center gap-[3px]">
        <span className="size-[5px] rounded-full bg-primary" />
        <span className="h-[3px] w-3 rounded-full bg-muted-foreground/50" />
      </span>
      <span className="absolute top-[9%] right-[8%] flex gap-[3px]">
        <span className="size-[4px] rounded-full bg-muted-foreground/40" />
        <span className="size-[4px] rounded-full bg-muted-foreground/40" />
      </span>
      <span className="absolute inset-x-0 top-[40%] flex flex-col items-center gap-[4px]">
        <span className="h-[4px] w-[46%] rounded-full bg-foreground/85" />
        <span className="h-[2.5px] w-[60%] rounded-full bg-muted-foreground/45" />
        <span className="mt-[3px] flex h-[11px] w-[70%] items-center justify-end rounded-[4px] border border-border-strong bg-card pr-[2px] shadow-[0_2px_6px_-2px_color-mix(in_srgb,var(--shadow-color)_25%,transparent)]">
          <span className="h-[6px] w-[12px] rounded-[2px] bg-secondary" />
        </span>
      </span>
    </span>
  );
}
