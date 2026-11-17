import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Folder, LoaderCircle, Minus, Plus, RefreshCw, RotateCcw } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/segmented";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet } from "@/components/ui/sheet";
import { Hint } from "@/components/ui/tooltip";
import { ipc, toAppError } from "@/lib/ipc";
import { TOOL_NAMES, TOOL_ORDER, TOOL_ROLES } from "@/lib/tools";
import { cn } from "@/lib/utils";
import { useAppUpdate } from "@/stores/appUpdate";
import { selectAvailable, useBinaries } from "@/stores/binaries";
import { useTheme } from "@/stores/theme";
import { useToasts } from "@/stores/toasts";
import { useUi } from "@/stores/ui";
import type { FilenameStyle, Settings, SettingsView } from "@/types";

const FILENAME_STYLES: { id: FilenameStyle; label: string; example: string }[] = [
  { id: "title_id", label: "Title [id]", example: "Big Buck Bunny [aqz-KE-bpKQ].mp4" },
  { id: "title", label: "Title", example: "Big Buck Bunny.mp4" },
  { id: "channel_title", label: "Channel - Title", example: "Blender - Big Buck Bunny.mp4" },
  { id: "date_title", label: "Date Title", example: "2014-11-10 Big Buck Bunny.mp4" },
  { id: "custom", label: "Custom…", example: "" },
];

/** Mirrors `MAX_CONCURRENT_LIMIT` in settings.rs. */
const MAX_CONCURRENT = 6;
const SETTINGS_KEY = ["settings"];

/** Preferences: where files go, how they are named, how many download at once, and updates. */
export function SettingsSheet() {
  const open = useUi((state) => state.panel === "settings");
  const closePanel = useUi((state) => state.closePanel);
  const queryClient = useQueryClient();

  const settings = useQuery({ queryKey: SETTINGS_KEY, queryFn: ipc.getSettings, staleTime: Number.POSITIVE_INFINITY });
  const save = useMutation({
    mutationFn: (next: Settings) => ipc.setSettings(next),
    onSuccess: (saved) => queryClient.setQueryData(SETTINGS_KEY, saved),
  });

  return (
    <Sheet open={open} onOpenChange={(next) => !next && closePanel()} title="Settings">
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

/** The backend wants plain settings back, without the display-only extras. */
function stripView({ default_output_dir: _default, ...settings }: SettingsView): Settings {
  return settings;
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
  // The custom template is saved when the field loses focus, not on every keystroke.
  const [template, setTemplate] = useState(settings.custom_template);
  useEffect(() => setTemplate(settings.custom_template), [settings.custom_template]);

  const chooseFolder = async () => {
    const folder = await openDialog({
      directory: true,
      defaultPath: settings.output_dir ?? settings.default_output_dir ?? undefined,
    });
    if (typeof folder === "string") onChange({ output_dir: folder });
  };

  const theme = useTheme((state) => state.theme);
  const setTheme = useTheme((state) => state.setTheme);

  const folder = settings.output_dir ?? settings.default_output_dir ?? "Downloads";
  const style = FILENAME_STYLES.find((candidate) => candidate.id === settings.filename);

  return (
    <div className="space-y-6 pt-1">
      <Group title="Downloads">
        <Row title="Save to" hint="Where finished downloads go.">
          <div className="mt-2.5 flex h-9 items-center gap-1 rounded-lg border border-border-strong bg-card pr-1 pl-3">
            <Folder className="size-3.5 shrink-0 text-subtle-foreground" />
            <span data-selectable title={folder} className="ml-1.5 min-w-0 flex-1 truncate text-[13px]">
              {folder}
            </span>
            {settings.output_dir && (
              <Hint label="Use the Downloads folder again">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Use the Downloads folder again"
                  onClick={() => onChange({ output_dir: null })}
                >
                  <RotateCcw className="size-3.5" />
                </Button>
              </Hint>
            )}
            <Button variant="ghost" size="sm" className="h-7 px-2.5 text-foreground" onClick={() => void chooseFolder()}>
              Change
            </Button>
          </div>
        </Row>

        <Row title="File names" hint="Clips cut from a video also get their time range added.">
          <Select value={settings.filename} onValueChange={(filename) => onChange({ filename: filename as FilenameStyle })}>
            <SelectTrigger className="mt-2.5">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {FILENAME_STYLES.map(({ id, label }) => (
                <SelectItem key={id} value={id}>
                  {label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {settings.filename === "custom" ? (
            <>
              <input
                value={template}
                onChange={(event) => setTemplate(event.target.value)}
                onBlur={() => template !== settings.custom_template && onChange({ custom_template: template })}
                onKeyDown={(event) => event.key === "Enter" && event.currentTarget.blur()}
                spellCheck={false}
                aria-label="Custom file name template"
                className="mt-2 h-9 w-full rounded-lg border border-border-strong bg-card px-3 font-mono text-xs outline-none transition-[border-color,box-shadow] duration-150 focus:border-primary/60 focus:shadow-[0_0_0_3px_var(--primary-soft)]"
              />
              <p className="mt-2 text-xs text-pretty text-muted-foreground">
                Uses yt-dlp’s fields, such as <Code>%(title)s</Code>, <Code>%(channel)s</Code>, <Code>%(id)s</Code> and{" "}
                <Code>%(upload_date)s</Code>. The file extension is added for you.
              </p>
            </>
          ) : (
            <p className="mt-2 flex min-w-0 items-baseline gap-2 text-xs">
              <span className="shrink-0 text-subtle-foreground">Example</span>
              <span data-selectable className="truncate font-mono text-[11px] text-muted-foreground">
                {style?.example}
              </span>
            </p>
          )}
          {error && (
            <p role="alert" className="mt-2 text-xs text-destructive">
              {error}
            </p>
          )}
        </Row>

        <Row
          title="At the same time"
          hint="How many downloads run at once. The rest wait their turn."
          control={
            <Stepper
              label="Simultaneous downloads"
              value={settings.max_concurrent}
              min={1}
              max={MAX_CONCURRENT}
              onChange={(max_concurrent) => onChange({ max_concurrent })}
            />
          }
        />
      </Group>

      <Group title="Appearance">
        <Row
          title="Theme"
          hint="System matches your computer’s light or dark mode."
          control={
            <Segmented
              label="Theme"
              group="theme"
              value={theme}
              options={[
                { id: "system", label: "System" },
                { id: "light", label: "Light" },
                { id: "dark", label: "Dark" },
              ]}
              onChange={setTheme}
            />
          }
        />
      </Group>

      <Group title="Updates">
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
      </Group>

      <Versions />
    </div>
  );
}

/** Installed versions of Siphon and its tools, with a manual check. */
function Versions() {
  const tools = useBinaries((state) => state.tools);
  const syncing = useBinaries((state) => state.syncing);
  const sync = useBinaries((state) => state.sync);
  // (A boolean, not the list: a selector must return a stable value.)
  const available = useBinaries((state) => selectAvailable(state).length > 0);
  const app = useAppUpdate();
  if (!tools) return null;

  const run = async () => {
    await Promise.all([sync(available), app.check()]);
    // A tool can't be swapped while a download is using it. Say so, or the
    // button would seem to have done nothing.
    const stillWaiting = selectAvailable(useBinaries.getState()).length > 0;
    if (available && stillWaiting) {
      useToasts.getState().show({
        id: "update-deferred",
        title: "The update is waiting for your downloads",
        description: "It will be installed once they have finished. Press Update now again then, or leave it to the next check.",
      });
    }
  };

  return (
    <Group
      title="Versions"
      hint="Checked at launch and once a day."
      action={
        <Button variant="ghost" size="sm" className="-mr-1 h-7 px-2" disabled={syncing} onClick={() => void run()}>
          {syncing ? <LoaderCircle className="size-3.5 animate-spin" /> : <RefreshCw className="size-3.5" />}
          {syncing ? "Checking…" : available ? "Update now" : "Check now"}
        </Button>
      }
    >
      <VersionRow name="Siphon" role="This app" current={app.current ?? "…"} next={app.available}>
        {app.available && (
          <Button size="sm" className="mt-2.5 w-full" disabled={app.installing} onClick={() => void app.install()}>
            Restart to update
          </Button>
        )}
      </VersionRow>
      {TOOL_ORDER.map((tool) => {
        const status = tools[tool];
        return (
          <VersionRow
            key={tool}
            name={TOOL_NAMES[tool]}
            // When a newer version waits, say what it changes instead.
            role={status.available ? status.available.summary : TOOL_ROLES[tool]}
            current={status.version ?? "not installed"}
            next={status.available?.version}
          />
        );
      })}
    </Group>
  );
}

/** One program: its name and what it does, with its version on the right. */
function VersionRow({
  name,
  role,
  current,
  next,
  children,
}: {
  name: string;
  role: string;
  current: string;
  next?: string | null;
  children?: ReactNode;
}) {
  return (
    <div className="px-3.5 py-2.5">
      <div className="flex items-baseline justify-between gap-3">
        <p className="shrink-0 text-[13px] font-medium">{name}</p>
        <span className="truncate text-xs text-muted-foreground tabular-nums">
          {current}
          {next && <span className="text-primary"> → {next}</span>}
        </span>
      </div>
      <p className="text-xs text-pretty text-muted-foreground">{role}</p>
      {children}
    </div>
  );
}

/** A titled card of settings rows. */
function Group({
  title,
  hint,
  action,
  children,
}: {
  title: string;
  hint?: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section>
      <div className="mb-2 flex min-h-7 items-end justify-between gap-3 px-1">
        <div className="min-w-0">
          <h3 className="text-xs font-semibold tracking-wide text-muted-foreground">{title}</h3>
          {hint && <p className="text-xs text-subtle-foreground">{hint}</p>}
        </div>
        {action}
      </div>
      <div className="divide-y rounded-xl border bg-card">{children}</div>
    </section>
  );
}

/**
 * One setting: its name, with a short explanation underneath that uses the
 * full width. A small control (`control`) sits on the name's line, to the
 * right; a wide one (`children`) goes below the explanation.
 */
function Row({
  title,
  hint,
  control,
  children,
}: {
  title: string;
  hint: string;
  control?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="px-3.5 py-3">
      <div className="flex items-center justify-between gap-3">
        <p className="min-w-0 truncate text-[13px] font-medium">{title}</p>
        {/* Taller than the line it sits on; the overlap goes into the row's padding. */}
        {control && <div className="-my-1.5 flex min-w-0 shrink-0 justify-end">{control}</div>}
      </div>
      <p className={cn("text-xs text-pretty text-muted-foreground", control ? "mt-1.5" : "mt-0.5")}>{hint}</p>
      {children}
    </div>
  );
}

/** A number with − and + buttons either side. */
function Stepper({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  const step = "grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-35";
  return (
    <div role="group" aria-label={label} className="flex shrink-0 items-center rounded-lg bg-accent p-0.5">
      <button type="button" aria-label="Fewer" disabled={value <= min} onClick={() => onChange(value - 1)} className={step}>
        <Minus className="size-3.5" />
      </button>
      <output aria-live="polite" className="w-7 text-center text-[13px] font-semibold tabular-nums">
        {value}
      </output>
      <button type="button" aria-label="More" disabled={value >= max} onClick={() => onChange(value + 1)} className={step}>
        <Plus className="size-3.5" />
      </button>
    </div>
  );
}

function Code({ children }: { children: ReactNode }) {
  return <code className="rounded bg-secondary px-1 py-0.5 font-mono text-[11px]">{children}</code>;
}
