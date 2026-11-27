import { ArrowDownToLine, Captions, CircleAlert, KeyRound, Languages, Palette, RefreshCw } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { Segmented } from "@/components/ui/segmented";
import { Sheet } from "@/components/ui/sheet";
import { toAppError } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { stripView, useSaveSettings, useSettings } from "@/stores/settings";
import { useTheme } from "@/stores/theme";
import { useUi } from "@/stores/ui";
import type { Settings, SettingsView } from "@/types";
import { FileNames, FolderTile, Lanes, SpeedLimit, ThemePicker } from "./controls";
import { Card, Reveal, Row, Section, type SectionId, SECTIONS } from "./parts";
import { MyLanguages } from "./MyLanguages";
import { SignedInSites } from "./SignedInSites";
import { SubtitleSync } from "./SubtitleSync";
import { Versions } from "./Versions";

/** Mirrors `MAX_CONCURRENT_LIMIT` in settings.rs. */
const MAX_CONCURRENT = 6;

/**
 * Preferences: where files go, how they are named, how many download at once,
 * signed-in sites, the theme, and updates. Tabs under the title follow the
 * section in view, and glide to one when clicked.
 */
export function SettingsSheet() {
  const open = useUi((state) => state.panel === "settings");
  const closePanel = useUi((state) => state.closePanel);
  const settings = useSettings();
  const save = useSaveSettings();
  const [body, setBody] = useState<HTMLDivElement | null>(null);
  const { active, jump } = useSectionSpy(body);

  return (
    <Sheet
      open={open}
      onOpenChange={(next) => !next && closePanel()}
      title="Settings"
      bodyRef={setBody}
      toolbar={<SectionTabs active={active} onPick={jump} />}
    >
      {settings.data && (
        <SettingsForm
          settings={settings.data}
          onChange={(changes) => save.mutate({ ...stripView(settings.data), ...changes })}
          error={save.isError ? toAppError(save.error).message : null}
        />
      )}
    </Sheet>
  );
}

function SettingsForm({
  settings,
  onChange,
  error,
}: {
  settings: SettingsView;
  onChange: (changes: Partial<Settings>) => void;
  error: string | null;
}) {
  const theme = useTheme((state) => state.theme);
  const setTheme = useTheme((state) => state.setTheme);

  return (
    <div className="space-y-7 pt-4">
      {/* A change the backend couldn't store has already gone back; this says why. */}
      <Reveal open={error !== null}>
        <p role="alert" className="flex gap-2 rounded-xl bg-destructive-soft px-3.5 py-2.5 text-xs text-destructive">
          <CircleAlert className="mt-px size-3.5 shrink-0" />
          <span>
            <span className="font-medium">That change couldn’t be saved.</span> {error}
          </span>
        </p>
      </Reveal>
      <Section id="downloads" index={0} icon={ArrowDownToLine} title="Downloads">
        <Card>
          <Row title="Save to">
            <FolderTile
              folder={settings.output_dir ?? settings.default_output_dir ?? "Downloads"}
              custom={settings.output_dir !== null}
              onChange={(output_dir) => onChange({ output_dir })}
            />
          </Row>
          <Row title="File names" hint="Clips cut from a video also get their time range added.">
            <FileNames style={settings.filename} onChange={(filename) => onChange({ filename })} />
          </Row>
          <Row
            title="At the same time"
            hint="How many downloads run at once. The rest wait their turn."
            control={
              <Lanes
                label="Simultaneous downloads"
                value={settings.max_concurrent}
                max={MAX_CONCURRENT}
                onChange={(max_concurrent) => onChange({ max_concurrent })}
              />
            }
          />
          <Row title="Speed limit" hint="For all downloads together, in MB/s. A change applies to running downloads at once.">
            <SpeedLimit limit={settings.speed_limit} onChange={(speed_limit) => onChange({ speed_limit })} />
          </Row>
        </Card>
      </Section>

      <Section
        id="languages"
        index={1}
        icon={Languages}
        title="Languages"
        hint="An MKV’s sound and subtitles in your languages are kept when you choose its tracks, and the first language’s sound plays first."
      >
        <Card>
          <Row title="My languages" hint="Best first.">
            <MyLanguages
              chosen={settings.languages}
              standIn={settings.default_languages}
              onChange={(languages) => onChange({ languages })}
            />
          </Row>
        </Card>
      </Section>

      <Section
        id="sites"
        index={2}
        icon={KeyRound}
        title="Signed-in sites"
        hint="Videos download without your account unless they need it."
      >
        <SignedInSites />
      </Section>

      <Section id="appearance" index={3} icon={Palette} title="Appearance">
        <Card>
          <Row title="Theme" hint="System follows Windows’ light or dark mode.">
            <ThemePicker value={theme} onChange={setTheme} />
          </Row>
        </Card>
      </Section>

      <Section id="sync" index={4} icon={Captions} title="Subtitle sync">
        <SubtitleSync />
      </Section>

      <Section id="updates" index={5} icon={RefreshCw} title="Updates">
        <Card>
          <Row
            title="yt-dlp releases"
            hint="Sites change often, and yt-dlp follows within days. Nightly gets those fixes sooner."
            control={
              <Segmented
                label="yt-dlp releases"
                group="channel"
                value={settings.update_channel}
                options={[
                  { id: "stable", label: "Stable" },
                  { id: "nightly", label: "Nightly" },
                ]}
                onChange={(update_channel) => onChange({ update_channel })}
              />
            }
          />
          <Row
            title="New versions"
            hint="Installed in the background, or only once you say so. Siphon itself always asks first."
            control={
              <Segmented
                label="New versions"
                group="update-mode"
                value={settings.update_mode}
                options={[
                  { id: "auto", label: "Automatic" },
                  { id: "ask", label: "Ask first" },
                ]}
                onChange={(update_mode) => onChange({ update_mode })}
              />
            }
          />
        </Card>
        <Versions />
      </Section>
    </div>
  );
}

/**
 * The panel's sections as tabs. An aqua mark glides under the one in view,
 * and a soft highlight follows the pointer.
 */
function SectionTabs({ active, onPick }: { active: SectionId; onPick: (id: SectionId) => void }) {
  const [pointed, setPointed] = useState<SectionId | null>(null);
  return (
    <nav
      aria-label="Settings sections"
      onPointerLeave={() => setPointed(null)}
      className="relative mx-5 flex border-b border-border-strong/70"
    >
      {SECTIONS.map(({ id, tab }) => {
        const current = id === active;
        return (
          <button
            key={id}
            type="button"
            aria-current={current || undefined}
            onClick={() => onPick(id)}
            onPointerEnter={() => setPointed(id)}
            className={cn(
              "relative h-9 flex-auto text-xs font-medium whitespace-nowrap outline-none transition-colors duration-200 focus-visible:text-primary",
              current ? "text-foreground" : "text-muted-foreground hover:text-foreground",
            )}
          >
            {pointed === id && (
              <motion.span
                layoutId="settings-tab-hover"
                transition={spring}
                className="absolute inset-x-0.5 inset-y-1 rounded-lg bg-accent"
              />
            )}
            <span className="relative">{tab}</span>
            {current && (
              <motion.span
                layoutId="settings-tab-mark"
                transition={spring}
                className="absolute inset-x-4 -bottom-px h-0.5 rounded-full bg-primary shadow-[0_0_10px_var(--primary)]"
              />
            )}
          </button>
        );
      })}
    </nav>
  );
}

/**
 * Which section is in view in the panel's scrolling body, and a way to glide
 * to one. A section is in view once its top passes the upper third; at the
 * very end, the last one is, however short. After a tab is clicked, that tab
 * stays chosen until the user scrolls themselves: a short last section can't
 * reach the top, and the mark would otherwise slide past it.
 */
function useSectionSpy(body: HTMLDivElement | null) {
  const [active, setActive] = useState<SectionId>(SECTIONS[0].id);
  const pinned = useRef(false);
  const reduceMotion = useReducedMotion();

  useEffect(() => {
    if (!body) {
      setActive(SECTIONS[0].id);
      return;
    }
    const spy = () => {
      if (pinned.current) return;
      const top = body.getBoundingClientRect().top;
      const sections = [...body.querySelectorAll<HTMLElement>("[data-section]")];
      const atEnd = body.scrollTop + body.clientHeight >= body.scrollHeight - 4;
      let current = sections[0];
      for (const section of sections) {
        if (section.getBoundingClientRect().top - top <= body.clientHeight / 3) current = section;
      }
      if (atEnd && body.scrollTop > 0) current = sections.at(-1);
      const id = current?.dataset.section as SectionId | undefined;
      if (id) setActive(id);
    };
    const unpin = () => {
      pinned.current = false;
    };
    body.addEventListener("scroll", spy, { passive: true });
    // Scrolling by hand: the wheel, a touch, the keyboard, or the scrollbar.
    for (const event of ["wheel", "touchstart", "keydown", "pointerdown"] as const) {
      body.addEventListener(event, unpin, { passive: true });
    }
    return () => {
      body.removeEventListener("scroll", spy);
      for (const event of ["wheel", "touchstart", "keydown", "pointerdown"] as const) {
        body.removeEventListener(event, unpin);
      }
    };
  }, [body]);

  const jump = (id: SectionId) => {
    const target = body?.querySelector<HTMLElement>(`[data-section="${id}"]`);
    if (!body || !target) return;
    pinned.current = true;
    setActive(id);
    const top = target.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop - 12;
    body.scrollTo({ top: Math.max(0, top), behavior: reduceMotion ? "auto" : "smooth" });
  };

  return { active, jump };
}
