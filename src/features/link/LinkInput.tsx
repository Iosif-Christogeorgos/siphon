import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { ClipboardPaste, FileMusic, Files, FileVideoCamera, FolderOpen, Link2, LoaderCircle, X } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { type ClipboardEvent, type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { Hint } from "@/components/ui/tooltip";
import { isFileKey } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { parseVideoUrl } from "@/lib/url";
import { cn } from "@/lib/utils";

interface LinkInputProps {
  /** The link currently loaded, if any. */
  url: string | null;
  /** A file on this PC loaded in place of a link (or several: `many`): the field shows it. */
  file?: { name: string; audio: boolean; many?: boolean } | null;
  loading: boolean;
  onSubmit: (url: string) => void;
  onClear: () => void;
  /** Browse for files on this PC to convert (the backend opens the dialog). */
  onBrowse: () => void;
}

/**
 * The large paste field. Pasting a link submits it; typing needs Enter.
 * Browse beside Paste picks files on this PC instead, which then show here.
 */
export function LinkInput({ url, file, loading, onSubmit, onClear, onBrowse }: LinkInputProps) {
  const [draft, setDraft] = useState(linkOf(url));
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const shake = useAnimationControls();

  // Follow links that arrive from elsewhere (clipboard offer, global paste).
  useEffect(() => setDraft(linkOf(url)), [url]);

  const submit = (text: string) => {
    const parsed = parseVideoUrl(text);
    if (parsed) {
      setInvalid(false);
      onSubmit(parsed);
      inputRef.current?.blur();
    } else if (text.trim()) {
      setInvalid(true);
      void shake.start({ x: [0, -7, 6, -4, 2, 0], transition: { duration: 0.36 } });
    }
  };

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault();
    submit(draft);
  };

  const handlePaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const parsed = parseVideoUrl(event.clipboardData.getData("text"));
    if (parsed) {
      event.preventDefault();
      setDraft(parsed);
      submit(parsed);
    }
  };

  const pasteFromClipboard = async () => {
    const text = await readText().catch(() => "");
    if (text) {
      setDraft(text.trim());
      submit(text);
    } else {
      inputRef.current?.focus();
    }
  };

  const clear = () => {
    setDraft("");
    setInvalid(false);
    onClear();
    inputRef.current?.focus();
  };

  return (
    <motion.form animate={shake} onSubmit={handleSubmit} className="relative">
      <div
        className={cn(
          "group flex h-14 items-center gap-3 rounded-2xl border bg-card pr-2.5 pl-4.5",
          // In focus: one crisp edge, a hairline of light around it and a soft glow, rather
          // than a wide band of tint, which read as a second, muddy outline. The same
          // layers at rest, so the light grows out of the edge instead of jumping on.
          "shadow-[0_0_0_0_transparent,0_0_0_0_transparent,var(--shadow-card)]",
          "transition-[border-color,box-shadow] duration-300 ease-[cubic-bezier(0.16,1,0.3,1)]",
          invalid
            ? "border-destructive/70 focus-within:shadow-[0_0_0_1px_color-mix(in_oklab,var(--destructive)_22%,transparent),0_0_36px_-6px_var(--destructive-glow),var(--shadow-card)]"
            : "focus-within:border-primary/85 focus-within:shadow-[0_0_0_1px_color-mix(in_oklab,var(--primary)_22%,transparent),0_0_36px_-6px_var(--primary-glow),var(--shadow-card)]",
        )}
      >
        <AnimatePresence mode="popLayout" initial={false}>
          {file ? (
            <motion.span key="file" {...swap} className="flex shrink-0 text-primary">
              {file.many ? (
                <Files className="size-4.5" />
              ) : file.audio ? (
                <FileMusic className="size-4.5" />
              ) : (
                <FileVideoCamera className="size-4.5" />
              )}
            </motion.span>
          ) : (
            <motion.span key="link" {...swap} className="flex shrink-0">
              <Link2
                className={cn(
                  "size-4.5 text-subtle-foreground transition-colors duration-300",
                  invalid ? "text-destructive" : "group-focus-within:text-primary",
                )}
              />
            </motion.span>
          )}
        </AnimatePresence>
        {file && (
          <motion.span
            key={file.name}
            data-selectable
            {...swap}
            className="min-w-0 flex-1 truncate text-[15px] font-medium"
            title={file.name}
          >
            {file.name}
          </motion.span>
        )}
        <input
          ref={inputRef}
          hidden={Boolean(file)}
          value={draft}
          onChange={(event) => {
            setDraft(event.target.value);
            setInvalid(false);
          }}
          onPaste={handlePaste}
          placeholder="Paste a video link"
          aria-label="Video link"
          aria-invalid={invalid}
          spellCheck={false}
          autoComplete="off"
          autoFocus
          className="h-full min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-subtle-foreground"
        />

        <AnimatePresence mode="wait" initial={false}>
          {loading ? (
            <Adornment key="loading">
              <LoaderCircle className="mr-2 size-4.5 animate-spin text-primary" />
            </Adornment>
          ) : draft || file ? (
            <Adornment key="clear">
              <Hint label="Clear">
                <button type="button" aria-label="Clear" onClick={clear} className={iconButton}>
                  <X className="size-4" />
                </button>
              </Hint>
            </Adornment>
          ) : (
            <Adornment key="paste">
              <Hint label="Convert video or sound files on this PC">
                <button
                  type="button"
                  onClick={onBrowse}
                  className="mr-1 flex h-9 items-center gap-1.5 rounded-xl px-3 text-[13px] font-medium text-muted-foreground transition-[background-color,color,transform] duration-150 hover:bg-accent hover:text-foreground active:scale-[0.97]"
                >
                  <FolderOpen className="size-3.5" />
                  Browse
                </button>
              </Hint>
              <button
                type="button"
                onClick={() => void pasteFromClipboard()}
                className="flex h-9 items-center gap-1.5 rounded-xl bg-secondary px-3 text-[13px] font-medium text-secondary-foreground transition-[background-color,transform] duration-150 hover:bg-border-strong active:scale-[0.97]"
              >
                <ClipboardPaste className="size-3.5" />
                Paste
              </button>
            </Adornment>
          )}
        </AnimatePresence>
      </div>

      {/* Takes up room as it opens, so whatever sits below moves down instead of being covered. */}
      <AnimatePresence initial={false}>
        {invalid && (
          <motion.div
            key="invalid"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.22, ease: [0.16, 1, 0.3, 1] }}
            className="overflow-hidden"
          >
            <p role="alert" className="pt-2 pl-4.5 text-[13px] text-destructive">
              That doesn’t look like a link. It should start with https://
            </p>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.form>
  );
}

/** The link to show: a file's key stands where a link would, but isn't one. */
const linkOf = (url: string | null) => (url && !isFileKey(url) ? url : "");

/** The link's icon and the file's, and the file's name, trading places. */
const swap = {
  initial: { opacity: 0, scale: 0.85 },
  animate: { opacity: 1, scale: 1 },
  exit: { opacity: 0, scale: 0.85 },
  transition: spring,
} as const;

const iconButton =
  "grid size-9 place-items-center rounded-xl text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground";

function Adornment({ children }: { children: ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.85 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.85 }}
      transition={spring}
      className="flex shrink-0 items-center"
    >
      {children}
    </motion.div>
  );
}
