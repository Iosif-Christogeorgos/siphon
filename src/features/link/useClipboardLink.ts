import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { useCallback, useEffect, useRef, useState } from "react";
import { parseVideoUrl } from "@/lib/url";

/**
 * Watches the clipboard whenever the window gains focus and offers any link
 * found there, unless it is the one already loaded or was dismissed.
 */
export function useClipboardLink(currentUrl: string | null) {
  const [offer, setOffer] = useState<string | null>(null);
  const dismissed = useRef<string | null>(null);
  const current = useRef(currentUrl);
  current.current = currentUrl;

  useEffect(() => {
    const check = async () => {
      // Reading fails when the clipboard holds something other than text.
      const link = parseVideoUrl(await readText().catch(() => null));
      const fresh = link !== null && link !== current.current && link !== dismissed.current;
      setOffer(fresh ? link : null);
    };
    void check();
    window.addEventListener("focus", check);
    return () => window.removeEventListener("focus", check);
  }, []);

  // Loading a link by any route makes an identical offer pointless.
  useEffect(() => {
    setOffer((link) => (link === currentUrl ? null : link));
  }, [currentUrl]);

  const dismiss = useCallback(() => {
    setOffer((link) => {
      dismissed.current = link;
      return null;
    });
  }, []);

  return { offer, dismiss };
}
