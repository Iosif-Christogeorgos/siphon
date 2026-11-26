import { useEffect, useId } from "react";
import { holdSynced } from "@/stores/subsync";
import { type Draft, syncedIn } from "./draft";

/**
 * Keeps the synced copies of subtitles these drafts use, and lets go of
 * them as soon as no draft does: undone, or the file closed. What no draft
 * and no unfinished copy uses is taken off the disk (`stores/subsync.ts`).
 */
export function useSyncedInUse(drafts: (Draft | null)[]) {
  const holder = useId();
  const keys = drafts
    .flatMap((draft) => (draft ? syncedIn(draft) : []))
    .sort()
    .join("\n");
  useEffect(() => holdSynced(holder, keys ? keys.split("\n") : []), [holder, keys]);
  useEffect(() => () => holdSynced(holder, []), [holder]);
}
