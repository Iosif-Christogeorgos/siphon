/**
 * Returns the trimmed text if it is a single http(s) URL, otherwise null.
 * yt-dlp supports thousands of sites, so we only check the shape, not the host.
 */
export function parseVideoUrl(text: string | null | undefined): string | null {
  const candidate = text?.trim();
  if (!candidate || /\s/.test(candidate) || candidate.length > 2048) return null;
  try {
    const url = new URL(candidate);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    if (!url.hostname.includes(".")) return null;
    return url.toString();
  } catch {
    return null;
  }
}

/**
 * For a YouTube video link that also names a playlist (`watch?v=…&list=…`),
 * the link to the whole playlist. Null for anything else.
 */
export function playlistUrlOf(url: string): string | null {
  try {
    const parsed = new URL(url);
    const list = parsed.searchParams.get("list");
    const isYoutube = /(^|\.)youtube\.com$/.test(parsed.hostname) || parsed.hostname === "youtu.be";
    if (!isYoutube || !list || parsed.pathname === "/playlist") return null;
    return `https://www.youtube.com/playlist?list=${encodeURIComponent(list)}`;
  } catch {
    return null;
  }
}

/** "www.youtube.com" → "youtube.com", for compact display. */
export function displayHost(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}
