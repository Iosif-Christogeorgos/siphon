import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { ClipboardPaste, Link2, LoaderCircle, X } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { type ClipboardEvent, type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { Hint } from "@/components/ui/tooltip";
import { spring } from "@/lib/motion";
import { parseVideoUrl } from "@/lib/url";
import { cn } from "@/lib/utils";

interface LinkInputProps {
  /** The link currently loaded, if any. */
  url: string | null;
  loading: boolean;
  onSubmit: (url: string) => void;
  onClear: () => void;
}

/** The large paste field. Pasting a link submits it; typing needs Enter. */
export function LinkInput({ url, loading, onSubmit, onClear }: LinkInputProps) {
  const [draft, setDraft] = useState(url ?? "");
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const shake = useAnimationControls();

  // Follow links that arrive from elsewhere (clipboard offer, global paste).
  useEffect(() => setDraft(url ?? ""), [url]);

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
          "group flex h-14 items-center gap-3 rounded-2xl border bg-card pr-2.5 pl-4.5 shadow-card transition-[border-color,box-shadow] duration-200",
          "focus-within:border-primary/60 focus-within:shadow-[0_0_0_4px_var(--primary-soft),var(--shadow-card)]",
          invalid && "border-destructive/60 focus-within:border-destructive/60",
        )}
      >
        <Link2 className="size-4.5 shrink-0 text-subtle-foreground transition-colors group-focus-within:text-primary" />
        <input
          ref={inputRef}
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
          ) : draft ? (
            <Adornment key="clear">
              <Hint label="Clear">
                <button type="button" aria-label="Clear" onClick={clear} className={iconButton}>
                  <X className="size-4" />
                </button>
              </Hint>
            </Adornment>
          ) : (
            <Adornment key="paste">
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

      <AnimatePresence>
        {invalid && (
          <motion.p
            role="alert"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0 }}
            className="absolute top-full left-4.5 mt-2 text-[13px] text-destructive"
          >
            That doesn’t look like a link. It should start with https://
          </motion.p>
        )}
      </AnimatePresence>
    </motion.form>
  );
}

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
