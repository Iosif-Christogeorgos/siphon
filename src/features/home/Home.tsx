import { useQuery } from "@tanstack/react-query";
import { ListVideo, RotateCw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";
import { ClipboardOffer } from "@/features/link/ClipboardOffer";
import { LinkInput } from "@/features/link/LinkInput";
import { useClipboardLink } from "@/features/link/useClipboardLink";
import { VideoCardSkeleton } from "@/features/video/VideoCard";
import { PlaylistPane } from "@/features/playlist/PlaylistPane";
import { VideoPane } from "@/features/video/VideoPane";
import { ipc, toAppError } from "@/lib/ipc";
import { gentleSpring, riseIn } from "@/lib/motion";
import { parseVideoUrl, playlistUrlOf } from "@/lib/url";
import { cn } from "@/lib/utils";

const isMac = navigator.userAgent.includes("Mac");

function Kbd({ children }: { children: string }) {
  return (
    <kbd className="rounded-md border border-border-strong bg-card px-1.5 py-0.5 font-sans text-[11px] font-medium text-muted-foreground">
      {children}
    </kbd>
  );
}

export function Home() {
  const [url, setUrl] = useState<string | null>(null);
  const clipboard = useClipboardLink(url);

  const media = useQuery({
    queryKey: ["media", url],
    queryFn: () => ipc.fetchMedia(url as string),
    enabled: url !== null,
  });

  // Ctrl/Cmd+V anywhere in the window loads the link, not just in the field.
  useEffect(() => {
    const onPaste = (event: ClipboardEvent) => {
      if (event.target instanceof HTMLInputElement) return;
      const link = parseVideoUrl(event.clipboardData?.getData("text"));
      if (link) setUrl(link);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, []);

  const empty = url === null;
  const playlistUrl = url ? playlistUrlOf(url) : null;

  return (
    <motion.div
      className="scrollbar-thin h-full overflow-y-auto"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
    >
      <div
        className={cn(
          "mx-auto flex min-h-full w-full max-w-[640px] flex-col px-6 pb-10",
          empty ? "justify-center" : "pt-4",
        )}
      >
        <AnimatePresence initial={false}>
          {empty && (
            <motion.div
              key="hero"
              className="overflow-hidden text-center"
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={gentleSpring}
            >
              <h1 className="text-[28px] leading-tight font-semibold tracking-[-0.03em]">
                What are we saving today?
              </h1>
              <p className="mt-2 pb-8 text-[15px] text-muted-foreground">
                Paste a link from YouTube, Vimeo, X, TikTok and thousands of other sites.
              </p>
            </motion.div>
          )}
        </AnimatePresence>

        {/* `layout` glides the field from the centre to the top once a link is loaded. */}
        <motion.div layout="position" transition={gentleSpring}>
          <LinkInput url={url} loading={media.isFetching} onSubmit={setUrl} onClear={() => setUrl(null)} />
        </motion.div>

        <motion.div layout="position" transition={gentleSpring}>
          <AnimatePresence initial={false}>
            {clipboard.offer && (
              <motion.div
                key={clipboard.offer}
                className="overflow-hidden"
                initial={{ height: 0, opacity: 0 }}
                animate={{ height: "auto", opacity: 1 }}
                exit={{ height: 0, opacity: 0 }}
                transition={gentleSpring}
              >
                <ClipboardOffer
                  url={clipboard.offer}
                  onAccept={() => setUrl(clipboard.offer)}
                  onDismiss={clipboard.dismiss}
                />
              </motion.div>
            )}
          </AnimatePresence>
        </motion.div>

        <AnimatePresence initial={false}>
          {empty && !clipboard.offer && (
            <motion.p
              key="hint"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1, transition: { delay: 0.3 } }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className="mt-4 text-center text-xs text-subtle-foreground"
            >
              or press <Kbd>{isMac ? "⌘" : "Ctrl"}</Kbd> <Kbd>V</Kbd> anywhere in the window
            </motion.p>
          )}
        </AnimatePresence>

        <AnimatePresence mode="wait">
          {!empty && (
            <motion.section
              key={`${url}:${media.status}`}
              variants={riseIn}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="mt-4 space-y-3"
            >
              {media.isPending ? (
                <>
                  <VideoCardSkeleton />
                  <div className="skeleton h-13 rounded-2xl" />
                </>
              ) : media.isError ? (
                <ErrorCard
                  error={toAppError(media.error)}
                  action={
                    <Button variant="secondary" size="sm" onClick={() => void media.refetch()}>
                      <RotateCw /> Retry
                    </Button>
                  }
                />
              ) : media.data.type === "video" ? (
                <>
                  <VideoPane video={media.data} />
                  {playlistUrl && (
                    <button
                      type="button"
                      onClick={() => setUrl(playlistUrl)}
                      className="mx-auto flex items-center gap-1.5 rounded-full px-3 py-1.5 text-[13px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    >
                      <ListVideo className="size-3.5" />
                      This video is part of a playlist. Show all of it
                    </button>
                  )}
                </>
              ) : (
                <PlaylistPane playlist={media.data} />
              )}
            </motion.section>
          )}
        </AnimatePresence>
      </div>
    </motion.div>
  );
}
