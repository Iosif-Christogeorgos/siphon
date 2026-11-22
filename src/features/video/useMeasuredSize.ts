import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { ipc } from "@/lib/ipc";
import type { Choice, PlanSummary, Trim } from "@/types";

/** How long a choice stays put before its size is measured: a trim's handles move many times a second. */
const SETTLE_MS = 800;

/**
 * Measures in the background what the plan could only estimate (see `crate::sizing`), once the choice has
 * settled, and plans again when something was found. Nothing waits for it: another choice, a download
 * (`paused`) or leaving the video stops it. True while it measures.
 */
export function useMeasuredSize(
  url: string,
  choice: Choice | null,
  trim: Trim | null,
  plan: PlanSummary | undefined,
  paused: boolean,
): boolean {
  const queryClient = useQueryClient();
  const key = JSON.stringify([choice, trim]);
  const [settled, setSettled] = useState<string | null>(null);
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(key), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [key]);

  const wanted = choice !== null && plan?.measurable === true && !paused && settled === key;
  const measure = useQuery({
    queryKey: ["measure", url, choice, trim],
    queryFn: async () => {
      const outcome = await ipc.measureSize(url, choice as Choice, trim);
      // Not finished: asked again when this choice is back.
      if (outcome === "stopped") throw new Error("stopped");
      return outcome;
    },
    enabled: wanted,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });

  const found = measure.data === "found";
  const foundAt = measure.dataUpdatedAt;
  useEffect(() => {
    if (found) void queryClient.invalidateQueries({ queryKey: ["plan", url] });
  }, [found, foundAt, queryClient, url]);

  // Nothing to measure for what is shown now: end what may still run for what was.
  useEffect(() => {
    if (!wanted) void ipc.stopMeasuring();
  }, [wanted]);
  useEffect(() => () => void ipc.stopMeasuring(), []);

  return wanted && measure.isFetching;
}
