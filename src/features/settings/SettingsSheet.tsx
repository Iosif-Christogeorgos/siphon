import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Chip } from "@/components/ui/chip";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet } from "@/components/ui/sheet";
import { ipc, toAppError } from "@/lib/ipc";
import { useUi } from "@/stores/ui";
import type { FilenameStyle, Settings, SettingsView } from "@/types";

const FILENAME_STYLES: { id: FilenameStyle; label: string; example: string }[] = [
  { id: "title_id", label: "Title [id]", example: "Big Buck Bunny [aqz-KE-bpKQ].mp4" },
  { id: "title", label: "Title", example: "Big Buck Bunny.mp4" },
  { id: "channel_title", label: "Channel - Title", example: "Blender - Big Buck Bunny.mp4" },
  { id: "date_title", label: "Date Title", example: "2014-11-10 Big Buck Bunny.mp4" },
  { id: "custom", label: "Custom…", example: "" },
];

const CONCURRENCY_CHOICES = [1, 2, 3, 4, 5, 6];
const SETTINGS_KEY = ["settings"];

/** Preferences: where files go, how they are named, how many download at once. */
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

  const style = FILENAME_STYLES.find((candidate) => candidate.id === settings.filename);

  return (
    <div className="space-y-6 pt-1">
      <Section title="Save to" hint="Where finished downloads go.">
        <div className="flex items-center gap-2">
          <p
            data-selectable
            title={settings.output_dir ?? settings.default_output_dir ?? ""}
            className="min-w-0 flex-1 truncate rounded-lg bg-secondary px-3 py-2 text-[13px]"
          >
            {settings.output_dir ?? settings.default_output_dir ?? "Downloads"}
          </p>
          <Button variant="outline" size="sm" onClick={() => void chooseFolder()}>
            <FolderOpen /> Choose
          </Button>
        </div>
        {settings.output_dir && (
          <button
            type="button"
            onClick={() => onChange({ output_dir: null })}
            className="mt-1.5 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
          >
            Use the Downloads folder again
          </button>
        )}
      </Section>

      <Section title="File names" hint="Clips cut from a video also get their time range added.">
        <Select value={settings.filename} onValueChange={(filename) => onChange({ filename: filename as FilenameStyle })}>
          <SelectTrigger>
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
            <p className="mt-1.5 text-xs text-muted-foreground">
              Uses yt-dlp’s fields, such as <Code>%(title)s</Code>, <Code>%(channel)s</Code>, <Code>%(id)s</Code> and{" "}
              <Code>%(upload_date)s</Code>. The file extension is added for you.
            </p>
          </>
        ) : (
          <p className="mt-1.5 truncate text-xs text-muted-foreground">For example: {style?.example}</p>
        )}
        {error && (
          <p role="alert" className="mt-1.5 text-xs text-destructive">
            {error}
          </p>
        )}
      </Section>

      <Section title="At the same time" hint="How many downloads run at once. The rest wait their turn.">
        <div role="radiogroup" aria-label="Simultaneous downloads" className="flex gap-0.5">
          {CONCURRENCY_CHOICES.map((count) => (
            <Chip
              key={count}
              group="concurrency"
              tone="soft"
              selected={settings.max_concurrent === count}
              onSelect={() => onChange({ max_concurrent: count })}
            >
              {count}
            </Chip>
          ))}
        </div>
      </Section>
    </div>
  );
}

function Section({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <section>
      <h3 className="text-[13px] font-medium">{title}</h3>
      <p className="mb-2.5 text-xs text-muted-foreground">{hint}</p>
      {children}
    </section>
  );
}

function Code({ children }: { children: ReactNode }) {
  return <code className="rounded bg-secondary px-1 py-0.5 font-mono text-[11px]">{children}</code>;
}
