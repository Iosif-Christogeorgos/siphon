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

/** "www.youtube.com" → "youtube.com", for compact display. */
export function displayHost(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}
