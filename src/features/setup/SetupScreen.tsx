import { Check, RotateCw, TriangleAlert } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { LogoTile } from "@/components/Logo";
import { Button } from "@/components/ui/button";
import { riseIn, spring } from "@/lib/motion";
import { TOOL_NAMES, TOOL_ORDER, TOOL_ROLES } from "@/lib/tools";
import { isUsable, useBinaries } from "@/stores/binaries";
import type { ToolStatus } from "@/types";

const TOOLS = TOOL_ORDER.map((tool) => ({ tool, name: TOOL_NAMES[tool], role: TOOL_ROLES[tool] }));

/** First-run screen: shown until every managed binary is installed. */
export function SetupScreen() {
  const tools = useBinaries((state) => state.tools);
  const syncing = useBinaries((state) => state.syncing);
  const sync = useBinaries((state) => state.sync);
  if (!tools) return null;

  const failed = !syncing && Object.values(tools).some((status) => status.state === "failed");

  return (
    <motion.div
      className="flex h-full flex-col items-center justify-center px-6 pb-16"
      initial="hidden"
      animate="visible"
      exit="exit"
      variants={{ visible: { transition: { staggerChildren: 0.06 } } }}
    >
      <motion.div variants={riseIn}>
        <LogoTile className="size-16" />
      </motion.div>
      <motion.h1 variants={riseIn} className="mt-6 text-xl font-semibold tracking-tight">
        Setting up Siphon
      </motion.h1>
      <motion.p variants={riseIn} className="mt-1.5 max-w-xs text-center text-muted-foreground">
        Fetching the engines that do the heavy lifting. This only happens once.
      </motion.p>

      <motion.ul
        variants={riseIn}
        className="mt-8 w-full max-w-sm overflow-hidden rounded-2xl border bg-card shadow-card"
      >
        {TOOLS.map(({ tool, name, role }) => (
          <ToolRow key={tool} name={name} role={role} status={tools[tool]} />
        ))}
      </motion.ul>

      <div className="mt-5 h-9">
        <AnimatePresence>
          {failed && (
            <motion.div variants={riseIn} initial="hidden" animate="visible" exit="exit">
              <Button variant="secondary" onClick={() => void sync()}>
                <RotateCw /> Try again
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </motion.div>
  );
}

function ToolRow({ name, role, status }: { name: string; role: string; status: ToolStatus }) {
  const done = isUsable(status);
  const failed = status.state === "failed";
  const downloaded = done || status.state === "verifying" || status.state === "installing";
  const fraction = status.state === "downloading" ? (status.progress ?? 0) : downloaded ? 1 : 0;

  return (
    <li className="relative flex items-center gap-3 border-b px-4 py-3.5 last:border-b-0">
      <div className="min-w-0 flex-1">
        <div className="font-medium">{name}</div>
        <div className="truncate text-[13px] text-muted-foreground">
          {failed ? status.error : role}
        </div>
      </div>

      <div className="grid h-6 min-w-12 place-items-center justify-end text-[13px] text-muted-foreground tabular-nums">
        <AnimatePresence mode="wait" initial={false}>
          <motion.span
            key={done ? "done" : failed ? "failed" : status.state}
            initial={{ opacity: 0, scale: 0.8 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.8 }}
            transition={spring}
            className="flex items-center"
          >
            {done ? (
              <Check className="size-4 text-success" strokeWidth={2.5} />
            ) : failed ? (
              <TriangleAlert className="size-4 text-destructive" />
            ) : (
              statusLabel(status)
            )}
          </motion.span>
        </AnimatePresence>
      </div>

      {/* Hairline progress along the bottom edge of the row. */}
      <motion.div
        className="absolute inset-x-0 bottom-0 h-0.5 origin-left bg-primary"
        initial={false}
        animate={{ scaleX: fraction, opacity: done || failed ? 0 : 1 }}
        transition={{ scaleX: { type: "spring", stiffness: 60, damping: 20 }, opacity: { delay: 0.3 } }}
      />
    </li>
  );
}

function statusLabel(status: ToolStatus): string {
  switch (status.state) {
    case "downloading":
      return `${Math.round((status.progress ?? 0) * 100)}%`;
    case "verifying":
      return "Verifying";
    case "installing":
      return "Installing";
    default:
      return "Waiting";
  }
}
