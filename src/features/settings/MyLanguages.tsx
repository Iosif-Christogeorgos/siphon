import { ChevronLeft, X } from "lucide-react";
import { motion } from "motion/react";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/select";
import { Hint } from "@/components/ui/tooltip";
import { COMMON_LANGUAGES, languageName } from "@/lib/languages";
import { spring } from "@/lib/motion";

/**
 * The user's languages, best first: an MKV's tracks in them are kept, and
 * the first one's sound plays first. Until chosen, the subtitle language and
 * Windows' languages stand in, said so.
 */
export function MyLanguages({
  chosen,
  standIn,
  onChange,
}: {
  /** The languages chosen; empty until some are. */
  chosen: string[];
  /** Those taken as the user's until then. */
  standIn: string[];
  onChange: (languages: string[]) => void;
}) {
  const list = chosen.length > 0 ? chosen : standIn;
  const others = COMMON_LANGUAGES.filter((language) => !list.includes(language)).sort((a, b) =>
    languageName(a).localeCompare(languageName(b)),
  );
  const earlier = (place: number) => {
    const next = [...list];
    [next[place - 1], next[place]] = [next[place] as string, next[place - 1] as string];
    onChange(next);
  };

  return (
    <div className="space-y-2">
      <ol className="flex flex-wrap items-center gap-2">
        {list.map((language, place) => {
          const name = languageName(language);
          return (
            <motion.li
              key={language}
              layout
              transition={spring}
              className="flex h-8 items-center gap-1.5 rounded-lg border border-border-strong bg-card pr-1 pl-2.5 text-[13px] font-medium shadow-[var(--card-edge),0_1px_2px_rgb(0_0_0/0.06)]"
            >
              <span className="grid size-4 place-items-center rounded-full bg-primary-soft text-[10px] font-semibold tabular-nums text-primary">
                {place + 1}
              </span>
              {name}
              {place > 0 && (
                <Hint label={`Put ${name} before ${languageName(list[place - 1] ?? "")}`}>
                  <button type="button" aria-label={`Put ${name} earlier`} onClick={() => earlier(place)} className={SMALL}>
                    <ChevronLeft className="size-3.5" />
                  </button>
                </Hint>
              )}
              {list.length > 1 && (
                <Hint label={`Remove ${name}`}>
                  <button
                    type="button"
                    aria-label={`Remove ${name}`}
                    onClick={() => onChange(list.filter((other) => other !== language))}
                    className={SMALL}
                  >
                    <X className="size-3.5" />
                  </button>
                </Hint>
              )}
            </motion.li>
          );
        })}
        <li>
          <Select value="" onValueChange={(language) => onChange([...list, language])}>
            <SelectTrigger aria-label="Add a language" className="h-8 w-auto gap-1.5 border-dashed bg-transparent text-muted-foreground hover:border-primary/40 hover:bg-primary-soft/40 hover:text-foreground data-[state=open]:border-primary/50 data-[state=open]:text-foreground">
              Add a language
            </SelectTrigger>
            <SelectContent className="max-h-72">
              {others.map((language) => (
                <SelectItem key={language} value={language}>
                  {languageName(language)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </li>
      </ol>
      {chosen.length === 0 && (
        <p className="text-xs text-pretty text-subtle-foreground">
          Taken from your subtitle language and Windows’ languages until you change them.
        </p>
      )}
    </div>
  );
}

const SMALL =
  "grid size-6 place-items-center rounded-md text-subtle-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40 focus-visible:outline-none";
