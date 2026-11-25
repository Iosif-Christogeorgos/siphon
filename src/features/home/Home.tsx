import { useQuery } from "@tanstack/react-query";
import { ListVideo, RotateCw } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { ErrorCard } from "@/components/ErrorCard";
import { Button } from "@/components/ui/button";
import { FilesPane } from "@/features/files/FilesPane";
import { ClipboardOffer } from "@/features/link/ClipboardOffer";
import { LinkInput } from "@/features/link/LinkInput";
import { useClipboardLink } from "@/features/link/useClipboardLink";
import { VideoCardSkeleton } from "@/features/video/VideoCard";
import { PlaylistPane } from "@/features/playlist/PlaylistPane";
import { VideoPane } from "@/features/video/VideoPane";
import { ipc, isFileKey, onFilesDropped, toAppError } from "@/lib/ipc";
import { gentleSpring, riseIn } from "@/lib/motion";
import { parseVideoUrl, playlistUrlOf } from "@/lib/url";
import { cn } from "@/lib/utils";
import { useToasts } from "@/stores/toasts";
import { useUi } from "@/stores/ui";
import type { Intake, LocalFile, Media } from "@/types";

const GREETINGS = [
  "Your move.",
  "Bring it on.",
  "Throw something at me.",
  "Let’s get to work.",
  "Hand it over.",
  "Make it play anywhere.",
  "Make it yours.",
  "What’s worth keeping?",
  "Ready to cook?",
  "Sup",
  "Lock in",
] as const;

/** Picked once, as the app starts: the same all session, another next time. */
const GREETING = GREETINGS[Math.floor(Math.random() * GREETINGS.length)];

function Kbd({ children }: { children: string }) {
  return (
    <kbd className="rounded-md border border-border-strong bg-card px-1.5 py-0.5 font-sans text-[11px] font-medium text-muted-foreground">
      {children}
    </kbd>
  );
}

/** Sound files, by extension: the field shows a file's kind before it is read. */
const SOUND = /\.(mp3|m4a|m4b|aac|opus|ogg|oga|flac|wav|wma|aiff?|alac|ape|wv|mka|ac3|eac3|dts|amr|caf|mp2)$/i;

/** What is said when nothing dropped or picked is a video or sound file. */
function nothingTaken(intake: Intake): string {
  const others = intake.left_out;
  return others > 0
    ? `${others} ${others === 1 ? "file that isn’t video or sound was" : "files that aren’t video or sound were"} left out.`
    : "Drop video or sound files, or a folder of them.";
}

export function Home() {
  // A link, or the key of a file on this PC (see `file`).
  const [url, setUrl] = useState<string | null>(null);
  // The file the key stands for, as the backend described it.
  const [file, setFile] = useState<LocalFile | null>(null);
  // Several files dropped or picked at once, chosen from in a list.
  const [batch, setBatch] = useState<Intake | null>(null);
  const clipboard = useClipboardLink(url);
  // The user asked to look this link up with the site's sign-in ("Try with your sign-in").
  const [withAccount, setWithAccount] = useState<string | null>(null);

  const media = useQuery({
    // A file dropped again after it changed is read again (its version is new).
    queryKey: ["media", url, withAccount === url, file?.version ?? null],
    queryFn: (): Promise<Media> =>
      isFileKey(url as string)
        ? ipc.readFile(url as string).then((video) => ({ type: "video", ...video }))
        : ipc.fetchMedia(url as string, withAccount === url),
    enabled: url !== null,
  });

  const openLink = (link: string | null) => {
    setFile(null);
    setBatch(null);
    setUrl(link);
  };

  // Files dropped on the window or picked with Browse: one opens like a video, several in a list.
  const takeIn = (intake: Intake) => {
    useUi.getState().closePanel();
    const first = intake.files[0];
    if (!first) {
      useToasts.getState().show({
        id: "nothing-taken",
        title: "No video or sound to convert",
        description: nothingTaken(intake),
      });
      return;
    }
    if (intake.files.length > 1) {
      setFile(null);
      setUrl(null);
      setBatch(intake);
      return;
    }
    setBatch(null);
    setFile(first);
    setUrl(first.key);
  };
  const browse = () => void ipc.pickFiles().then((intake) => intake && takeIn(intake), () => {});

  // Ctrl/Cmd+V anywhere in the window loads the link, not just in the field.
  useEffect(() => {
    const onPaste = (event: ClipboardEvent) => {
      if (event.target instanceof HTMLInputElement) return;
      const link = parseVideoUrl(event.clipboardData?.getData("text"));
      if (link) openLink(link);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, []);

  // Files dropped anywhere on the window (the backend took them in, see `DropTarget`).
  useEffect(() => {
    const unlisten = onFilesDropped(takeIn);
    return () => void unlisten.then((stop) => stop());
  }, []);

  const empty = url === null && batch === null;
  const playlistUrl = url ? playlistUrlOf(url) : null;

  return (
    <motion.div
      // A stable gutter: the column doesn't shift sideways when the scrollbar appears.
      className="scrollbar-thin h-full overflow-y-auto [scrollbar-gutter:stable_both-edges]"
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
              <h1 className="text-[28px] leading-tight font-semibold tracking-[-0.03em]">{GREETING}</h1>
              <p className="mt-2 pb-8 text-[15px] text-pretty text-muted-foreground">
                Paste a link from YouTube, TikTok and thousands of other sites, or drop a file.
              </p>
            </motion.div>
          )}
        </AnimatePresence>

        {/* `layout` glides the field from the centre to the top once a link is loaded. */}
        <motion.div layout="position" transition={gentleSpring}>
          <LinkInput
            url={url}
            file={
              batch
                ? { name: `${batch.files.length} files`, audio: false, many: true }
                : file && { name: file.name, audio: SOUND.test(file.name) }
            }
            loading={media.isFetching}
            onSubmit={openLink}
            onClear={() => openLink(null)}
            onBrowse={browse}
          />
        </motion.div>

        <motion.div layout="position" transition={gentleSpring}>
          <AnimatePresence initial={false}>
            {clipboard.offer && (
              <motion.div
                key={clipboard.offer}
                // Clipped only while the height animates; once open, the pill's shadow shows in full.
                initial={{ height: 0, opacity: 0, overflow: "hidden" }}
                animate={{ height: "auto", opacity: 1, transitionEnd: { overflow: "visible" } }}
                exit={{ height: 0, opacity: 0, overflow: "hidden" }}
                transition={gentleSpring}
              >
                <ClipboardOffer
                  url={clipboard.offer}
                  onAccept={() => openLink(clipboard.offer)}
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
              or press <Kbd>Ctrl</Kbd> <Kbd>V</Kbd> anywhere in the window
            </motion.p>
          )}
        </AnimatePresence>

        <AnimatePresence mode="wait">
          {batch && (
            <motion.section
              key={`files:${batch.files.map((each) => each.key).join(",")}`}
              variants={riseIn}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="mt-4 space-y-3"
            >
              <FilesPane intake={batch} />
            </motion.section>
          )}
          {!empty && !batch && (
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
                  // Signed in: look the video up again, with the new sign-in.
                  signIn={
                    url
                      ? { url, onSignedIn: () => void media.refetch(), onUseAccount: () => setWithAccount(url) }
                      : undefined
                  }
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
                      onClick={() => openLink(playlistUrl)}
                      className="mx-auto flex items-center gap-1.5 rounded-full px-3 py-1.5 text-[13px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    >
                      <ListVideo className="size-3.5" />
                      This video is part of a playlist. Show all of it
                    </button>
                  )}
                </>
              ) : (
                <PlaylistPane playlist={media.data} onOpen={openLink} />
              )}
            </motion.section>
          )}
        </AnimatePresence>
      </div>
    </motion.div>
  );
}
