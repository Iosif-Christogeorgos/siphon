import type { Tool } from "@/types";

/** The managed tools, in the order they are listed. */
export const TOOL_ORDER: Tool[] = ["yt_dlp", "ffmpeg", "deno"];

export const TOOL_NAMES: Record<Tool, string> = {
  yt_dlp: "yt-dlp",
  ffmpeg: "FFmpeg",
  deno: "Deno",
};

/** What each tool is for, in a few words. */
export const TOOL_ROLES: Record<Tool, string> = {
  yt_dlp: "Finds and downloads the video",
  ffmpeg: "Merges and converts",
  deno: "Runs the scripts YouTube needs",
};
