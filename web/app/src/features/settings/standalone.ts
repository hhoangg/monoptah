import { useEffect, useState } from "react";

const QUERY = "(display-mode: standalone)";

/** True when the page runs as an installed app: `display-mode: standalone` or iOS `navigator.standalone`. */
export function isStandalone(): boolean {
  if (typeof window === "undefined") return false;
  if ((navigator as Navigator & { standalone?: boolean }).standalone === true) return true;
  return typeof window.matchMedia === "function" && window.matchMedia(QUERY).matches;
}

/** Reactive `isStandalone()`; follows the display mode changing while the page is open. */
export function useStandalone(): boolean {
  const [standalone, setStandalone] = useState(isStandalone);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia(QUERY);
    const update = () => setStandalone(isStandalone());
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  return standalone;
}

export type InstallPlatform = "ios" | "android" | "desktop";

/** Which guide to show first. iPadOS 13+ reports itself as a Mac with touch points. */
export function detectPlatform(userAgent = navigator.userAgent, maxTouchPoints = navigator.maxTouchPoints): InstallPlatform {
  if (/iPhone|iPad|iPod/.test(userAgent) || (/Macintosh/.test(userAgent) && maxTouchPoints > 1)) return "ios";
  if (/Android/.test(userAgent)) return "android";
  return "desktop";
}
