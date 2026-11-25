import { useDownloads } from "@/stores/downloads";
import type { DownloadEvent } from "@/types";
import { formatBytes } from "./format";
import { ipc, isFileKey } from "./ipc";
import { formatShortTimecode } from "./time";

/** Jobs that end within this long of each other share one notification. */
const GATHER_MS = 2000;

let finished: string[] = [];
let failed: string[] = [];
let timer: number | undefined;
/** Recordings waiting for their stream, to say when it went live. */
const waiting = new Set<string>();

/**
 * Collects finished and failed downloads into one Windows notification, sent
 * a moment after the last of them, and says at once when a stream that a
 * recording waited for goes live. The backend shows them only while Siphon
 * isn't the active window.
 */
export function announce(event: DownloadEvent) {
  if (event.type === "progress") {
    if (event.stage === "waiting_live") waiting.add(event.id);
    else if (event.stage === "recording" && waiting.delete(event.id)) wentLive(event.id);
    return;
  }
  waiting.delete(event.id);
  if (event.type !== "finished" && event.type !== "failed") return;
  (event.type === "finished" ? finished : failed).push(event.id);
  window.clearTimeout(timer);
  timer = window.setTimeout(send, GATHER_MS);
}

function wentLive(id: string) {
  const download = useDownloads.getState().byId[id];
  if (download) void ipc.notify("Stream is live, recording", download.title).catch(() => {});
}

function send() {
  const { byId } = useDownloads.getState();
  const named = (ids: string[]) => ids.map((id) => byId[id]).filter((download) => download !== undefined);
  const done = named(finished);
  const lost = named(failed);
  finished = [];
  failed = [];
  if (done.length + lost.length === 0) return;

  const name = (download: (typeof done)[number]) =>
    download.trim
      ? `${download.title} (${formatShortTimecode(download.trim.start)}–${formatShortTimecode(download.trim.end)})`
      : download.title;
  const list = (items: typeof done) =>
    items.length <= 2 ? items.map(name).join("\n") : `${items.slice(0, 2).map(name).join("\n")}\nand ${items.length - 2} more`;
  // "Recording finished" when everything in it was a recording, "Conversion" a file
  // converted, "Copy" an MKV's chosen tracks copied, "Download" a download; several
  // of different kinds are just "finished".
  const kind = (download: (typeof done)[number]) =>
    download.record ? "recording" : download.tracks ? "copy" : isFileKey(download.url) ? "conversion" : "download";
  const one = (items: typeof done) => {
    const first = items[0] ? kind(items[0]) : "download";
    return items.every((download) => kind(download) === first) ? first : null;
  };
  const noun = (items: typeof done) => {
    const word = one(items) ?? "download";
    return word.charAt(0).toUpperCase() + word.slice(1);
  };
  const nouns = (items: typeof done) => {
    const word = one(items);
    return word ? `${word === "copy" ? "copies" : `${word}s`} ` : "";
  };

  let title: string;
  let body: string;
  if (lost.length === 0) {
    const only = done.length === 1 ? done[0] : undefined;
    title = only ? `${noun(done)} finished` : `${done.length} ${nouns(done)}finished`;
    body = only ? [name(only), only.size != null && formatBytes(only.size)].filter(Boolean).join(" · ") : list(done);
  } else if (done.length === 0) {
    const only = lost.length === 1 ? lost[0] : undefined;
    title = only ? (only.record ? "Recording stopped" : `${noun(lost)} failed`) : `${lost.length} ${nouns(lost)}failed`;
    body = only ? `${name(only)}\n${only.error?.message ?? ""}`.trim() : list(lost);
  } else {
    title = `${done.length} finished, ${lost.length} failed`;
    body = `Failed: ${list(lost)}`;
  }
  void ipc.notify(title, body).catch(() => {});
}
