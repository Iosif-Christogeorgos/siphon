import { CircleAlert, X } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { Dialog } from "radix-ui";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { Segmented } from "@/components/ui/segmented";
import { toAppError } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { useAppUpdate } from "@/stores/appUpdate";
import { selectAvailable, useBinaries } from "@/stores/binaries";
import { stripView, useSaveSettings, useSettings } from "@/stores/settings";
import { useTheme } from "@/stores/theme";
import { type SettingsSection, useUi } from "@/stores/ui";
import type { Settings, SettingsView } from "@/types";
import { FileNames, FolderTile, Lanes, SpeedLimit, ThemePicker } from "./controls";
import { MyLanguages } from "./MyLanguages";
import { Card, GROUPS, ORDER, Reveal, Row, SECTIONS } from "./parts";
import { SignedInSites } from "./SignedInSites";
import { SubtitleSync } from "./SubtitleSync";
import { Versions } from "./Versions";

/** Mirrors `MAX_CONCURRENT_LIMIT` in settings.rs. */
const MAX_CONCURRENT = 6;

/**
 * Settings fill the window under the title bar (2026-11-27, the owner's pick
 * of three prototypes): a sidebar of pages grouped by what they are for, and
 * one page at a time beside it, each setting's name and explanation reading
 * across to its control once there is room.
 *
 * Not modal, as the slide-over panels aren't: the title bar (minimize, close,
 * dragging the window, Activity) and toasts stay in reach. Escape closes it.
 */
export function SettingsPanel() {
  const open = useUi((state) => state.panel === "settings");
  const closePanel = useUi((state) => state.closePanel);

  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && closePanel()} modal={false}>
      <AnimatePresence>
        {open && (
          <Dialog.Portal forceMount>
            <Dialog.Content
              asChild
              aria-describedby={undefined}
              // Focus the page itself, not its first control: no focus ring flashes on open.
              onOpenAutoFocus={(event) => {
                event.preventDefault();
                (event.currentTarget as HTMLElement).focus();
              }}
              onInteractOutside={(event) => {
                if ((event.target as Element | null)?.closest?.("[data-beside-panels]")) event.preventDefault();
              }}
            >
              <motion.div
                className="fixed inset-x-0 top-11 bottom-0 z-40 grid grid-cols-[13.5rem_minmax(0,1fr)] bg-background outline-none min-[860px]:grid-cols-[15rem_minmax(0,1fr)]"
                initial={{ opacity: 0, scale: 1.012 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 1.008 }}
                transition={{ ...spring, opacity: { duration: 0.18 } }}
              >
                <Sidebar />
                <Page />
              </motion.div>
            </Dialog.Content>
          </Dialog.Portal>
        )}
      </AnimatePresence>
    </Dialog.Root>
  );
}

/**
 * The pages, grouped. The one shown sits on a raised key with an aqua light
 * at its edge, which glides to the next; the arrow keys move between pages.
 */
function Sidebar() {
  const section = useUi((state) => state.settingsSection);
  const show = useUi((state) => state.showSettingsSection);
  // A newer Siphon or tool waiting: a light on Updates, so it is found without opening it.
  const updates = useAppUpdate((state) => state.available !== null);
  const tools = useBinaries((state) => selectAvailable(state).length > 0);

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    const step = { ArrowDown: 1, ArrowUp: -1 }[event.key];
    if (!step) return;
    event.preventDefault();
    const next = ORDER[(ORDER.indexOf(section) + step + ORDER.length) % ORDER.length] as SettingsSection;
    show(next);
    event.currentTarget.querySelector<HTMLElement>(`[data-page="${next}"]`)?.focus();
  };

  return (
    <nav
      aria-label="Settings pages"
      onKeyDown={onKeyDown}
      className="relative flex min-h-0 flex-col overflow-y-auto border-r bg-sheet px-3 pt-5 pb-4 scrollbar-thin"
    >
      {/* The same faint wash of aqua as behind the top of the window. */}
      <div className="pointer-events-none absolute inset-x-0 top-0 h-48 bg-[radial-gradient(100%_100%_at_0%_0%,var(--primary-soft),transparent)]" />
      <Dialog.Title className="relative mb-5 px-2.5 text-[17px] font-semibold tracking-tight">Settings</Dialog.Title>
      {GROUPS.map((group) => (
        <div key={group.title} role="group" aria-label={group.title} className="relative mb-4 space-y-0.5">
          <p aria-hidden className="mb-1.5 px-2.5 text-[11.5px] font-medium text-subtle-foreground">
            {group.title}
          </p>
          {group.sections.map((id) => {
            const { title, icon: Icon } = SECTIONS[id];
            const current = id === section;
            return (
              <button
                key={id}
                type="button"
                data-page={id}
                aria-current={current ? "page" : undefined}
                tabIndex={current ? 0 : -1}
                onClick={() => show(id)}
                className={cn(
                  "group/page relative flex h-[34px] w-full items-center gap-2.5 rounded-[9px] px-2.5 text-left text-[13px] font-medium outline-none transition-[color,background-color,transform] duration-150 focus-visible:ring-[3px] focus-visible:ring-ring/40 active:scale-[0.985]",
                  current ? "text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground",
                )}
              >
                {current && (
                  <motion.span
                    layoutId="settings-page"
                    transition={spring}
                    className="absolute inset-0 rounded-[9px] bg-popover shadow-[var(--card-edge),0_0_0_1px_var(--border),0_1px_3px_rgb(0_0_0/0.14)]"
                  >
                    <span className="absolute top-1/2 -left-3 h-[18px] w-[3px] -translate-y-1/2 rounded-r-full bg-primary shadow-[0_0_10px_var(--primary-glow)]" />
                  </motion.span>
                )}
                <Icon
                  className={cn(
                    "relative size-[15px] shrink-0 transition-colors duration-150",
                    current ? "text-primary" : "text-subtle-foreground group-hover/page:text-foreground",
                  )}
                  strokeWidth={1.9}
                />
                <span className="relative min-w-0 flex-1 truncate">{title}</span>
                {id === "updates" && (updates || tools) && (
                  <span
                    aria-label="An update is ready"
                    className="relative size-1.5 shrink-0 rounded-full bg-primary shadow-[0_0_8px_var(--primary-glow)]"
                  />
                )}
              </button>
            );
          })}
        </div>
      ))}
      <p className="relative mt-auto flex items-center gap-1.5 px-2.5 pt-2 text-xs text-subtle-foreground">
        <kbd className="rounded-md border border-border-strong bg-card px-1.5 py-px font-sans text-[11px] font-medium text-muted-foreground shadow-[0_1px_0_var(--border-strong)]">
          Esc
        </kbd>
        to close
      </p>
    </nav>
  );
}

/**
 * The page shown, under its title and what it is about. A change of page
 * slides the old one out and the new one in, from the side of the sidebar it
 * came from, and starts at its top.
 */
function Page() {
  const section = useUi((state) => state.settingsSection);
  const settings = useSettings();
  const save = useSaveSettings();
  const scroller = useRef<HTMLDivElement>(null);
  const reduceMotion = useReducedMotion();
  // Which way the page moves: down the sidebar, the new page rises from below.
  const [{ shown, direction }, setPaging] = useState({ shown: section, direction: 1 });
  if (shown !== section) setPaging({ shown: section, direction: ORDER.indexOf(section) > ORDER.indexOf(shown) ? 1 : -1 });
  useEffect(() => {
    scroller.current?.scrollTo({ top: 0 });
  }, [section]);

  const { title, description } = SECTIONS[section];
  const shift = reduceMotion ? 0 : 1;

  return (
    <div className="relative min-h-0">
      <Dialog.Close
        aria-label="Close"
        className="absolute top-4 right-4 z-10 grid size-8 place-items-center rounded-lg text-muted-foreground outline-none transition-[background-color,color,transform] duration-150 hover:bg-accent hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/40 active:scale-95"
      >
        <X className="size-4" />
      </Dialog.Close>
      <div ref={scroller} className="h-full overflow-y-auto scrollbar-thin">
        <div className="@container/settings mx-auto max-w-[44rem] px-6 pt-9 pb-14 min-[860px]:px-10">
          {/* A change the backend couldn't store has already gone back; this says why. */}
          <Reveal open={save.isError}>
            <p role="alert" className="mb-5 flex gap-2 rounded-xl bg-destructive-soft px-3.5 py-2.5 text-xs text-destructive">
              <CircleAlert className="mt-px size-3.5 shrink-0" />
              <span>
                <span className="font-medium">That change couldn’t be saved.</span>{" "}
                {save.isError ? toAppError(save.error).message : null}
              </span>
            </p>
          </Reveal>
          <AnimatePresence mode="wait" initial={false} custom={direction}>
            <motion.section
              key={section}
              aria-labelledby={`settings-${section}-title`}
              custom={direction}
              variants={{
                enter: (dir: number) => ({ opacity: 0, y: 14 * dir * shift, filter: "blur(3px)" }),
                center: { opacity: 1, y: 0, filter: "blur(0px)", transition: spring },
                leave: (dir: number) => ({
                  opacity: 0,
                  y: -8 * dir * shift,
                  filter: "blur(3px)",
                  transition: { duration: 0.11, ease: "easeIn" },
                }),
              }}
              initial="enter"
              animate="center"
              exit="leave"
            >
              <header className="mb-6 pr-10">
                <h2 id={`settings-${section}-title`} className="text-2xl leading-8 font-semibold tracking-tight">
                  {title}
                </h2>
                <p className="mt-1 text-[13px] text-pretty text-muted-foreground">{description}</p>
              </header>
              {settings.data && (
                <Contents
                  section={section}
                  settings={settings.data}
                  onChange={(changes) => settings.data && save.mutate({ ...stripView(settings.data), ...changes })}
                />
              )}
            </motion.section>
          </AnimatePresence>
        </div>
      </div>
    </div>
  );
}

/** What each page holds. */
function Contents({
  section,
  settings,
  onChange,
}: {
  section: SettingsSection;
  settings: SettingsView;
  onChange: (changes: Partial<Settings>) => void;
}) {
  const theme = useTheme((state) => state.theme);
  const setTheme = useTheme((state) => state.setTheme);

  switch (section) {
    case "downloads":
      return (
        <Card>
          <Row title="Save to" hint="Downloads and the clips cut from them go here.">
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
      );
    case "sites":
      return <SignedInSites />;
    case "languages":
      return (
        <Card>
          <Row
            title="My languages"
            hint="When you choose an MKV’s tracks, its sound and subtitles in these languages are kept, and the first language’s sound plays first."
          >
            <MyLanguages
              chosen={settings.languages}
              standIn={settings.default_languages}
              onChange={(languages) => onChange({ languages })}
            />
          </Row>
        </Card>
      );
    case "sync":
      return <SubtitleSync />;
    case "appearance":
      return (
        <Card>
          <Row title="Theme" hint="System follows Windows’ light or dark mode.">
            <ThemePicker value={theme} onChange={setTheme} />
          </Row>
        </Card>
      );
    case "updates":
      return (
        <div className="space-y-3">
          <Card>
            <Row
              title="yt-dlp releases"
              hint="Sites change often, and yt-dlp follows within days. Nightly gets those fixes sooner."
              control={
                <Segmented
                  label="yt-dlp releases"
                  group="channel"
                  variant="tray"
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
                  variant="tray"
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
        </div>
      );
  }
}
