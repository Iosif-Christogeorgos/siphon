import { useQuery } from "@tanstack/react-query";
import { Check, FolderOpen, List, Play } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useShallow } from "zustand/react/shallow";
import { Button } from "@/components/ui/button";
import { formatBytes, formatDay } from "@/lib/format";
import { ipc } from "@/lib/ipc";
import { spring } from "@/lib/motion";
import { formatShortTimecode } from "@/lib/time";
import { type Download, isActive, useDownloads } from "@/stores/downloads";
import { useUi } from "@/stores/ui";

/**
 * Says so when the open video was downloaded before (and the file is still
 * there), or is already downloading from elsewhere, such as a playlist.
 * Downloads started from this screen (`exclude`) don't count: the Download
 * button already shows those.
 */
export function AlreadyDownloaded({ url, exclude }: { url: string; exclude: string[] }) {
  const matches = useDownloads(
    useShallow((state) =>
      state.order
        .map((id) => state.byId[id])
        .filter((download): download is Download => download?.url === url && !exclude.includes(download.id)),
    ),
  );
  const finished = matches.filter((download) => download.status === "finished" && download.path).reverse();
  const elsewhere = matches.some((download) => isActive(download) || download.status === "paused");

  // Only files still where Siphon put them: a deleted or moved one is no reason to stop.
  const paths = finished.map((download) => download.path as string);
  const present = useQuery({
    queryKey: ["files-present", paths],
    queryFn: () => ipc.filesPresent(paths),
    enabled: paths.length > 0,
    staleTime: 0,
  });
  const kept = present.data ? finished.filter((_, index) => present.data[index]) : [];
  // The whole video if there is one, newest first; otherwise the clips.
  const whole = kept.find((download) => download.trim === null);
  const clips = kept.filter((download) => download.trim !== null);
  const lead = whole ?? clips[0];

  let notice: { text: string; detail: string | null; path: string | null } | null = null;
  if (lead) {
    const one = whole !== undefined || clips.length === 1;
    const clip = lead.trim && `${formatShortTimecode(lead.trim.start)}–${formatShortTimecode(lead.trim.end)}`;
    notice = {
      text: whole ? "You already have this" : one ? `You already have a clip of this (${clip})` : `You already have ${clips.length} clips of this`,
      detail: [lead.finishedAt && formatDay(lead.finishedAt), one && lead.size != null && formatBytes(lead.size)]
        .filter(Boolean)
        .join(" · "),
      path: one ? lead.path : null,
    };
  }

  const show = notice !== null || elsewhere;
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.div
          key="already"
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: "auto" }}
          exit={{ opacity: 0, height: 0 }}
          transition={spring}
          className="overflow-hidden"
        >
          <div className="flex h-11 items-center gap-2.5 rounded-xl border bg-card pr-1.5 pl-3.5 text-[13px] shadow-card">
            <Check className="size-4 shrink-0 text-success" />
            <p className="min-w-0 flex-1 truncate">
              <span className="font-medium">{notice ? notice.text : "Already in your downloads"}</span>
              {notice?.detail && <span className="ml-2 text-muted-foreground tabular-nums">{notice.detail}</span>}
            </p>
            {notice ? (
              <>
                <Button variant="ghost" size="sm" onClick={() => lead?.path && void ipc.revealFile(lead.path)}>
                  <FolderOpen /> Show in folder
                </Button>
                {notice.path && (
                  <Button variant="ghost" size="sm" onClick={() => notice.path && void ipc.openFile(notice.path)}>
                    <Play /> Open
                  </Button>
                )}
              </>
            ) : (
              <Button variant="ghost" size="sm" onClick={() => useUi.getState().openPanel("downloads")}>
                <List /> Show downloads
              </Button>
            )}
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
