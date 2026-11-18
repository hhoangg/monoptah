// F-14: the last place of the last used desktop, so `/` can bring the user back. Ported from
// mobile/src/features/workspace/lastPlace.ts (there a project or Mono id, here a route path).
import { useEffect } from "react";
import { useLocation } from "@tanstack/react-router";

export const LAST_DESKTOP_KEY = "monoptah.lastDesktop";
export const lastPlaceKey = (desktopId: string) => `monoptah.lastPlace.${desktopId}`;

// Settings and Cloud are not places to be brought back to; the workspace screens are.
const REMEMBERED = /^\/d\/([^/?#]+)(\/?$|\/(p|s|monos)\/)/;

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** The desktop id of a `/d/:id/...` path that is a workspace place, else null. */
export function placeDesktop(path: string): string | null {
  const match = REMEMBERED.exec(path);
  if (!match?.[1]) return null;
  try {
    return decodeURIComponent(match[1]);
  } catch {
    return null;
  }
}

/** Remembers `path` (with its search and hash) as the last place of its desktop and the desktop as the last used. */
export function rememberPlace(path: string): void {
  const desktopId = placeDesktop(path);
  const store = storage();
  if (!desktopId || !store) return;
  try {
    store.setItem(LAST_DESKTOP_KEY, desktopId);
    store.setItem(lastPlaceKey(desktopId), path);
  } catch {
    // Storage full or blocked: the next visit simply has no last place.
  }
}

/** Remembers `desktopId` as the last used desktop, whatever screen of it is open (settings and cloud included). */
export function rememberDesktop(desktopId: string): void {
  const store = storage();
  if (!desktopId || !store) return;
  try {
    store.setItem(LAST_DESKTOP_KEY, desktopId);
  } catch {
    // Storage full or blocked: the next visit simply has no last desktop.
  }
}

/** The path to restore: the last place of the last used desktop, or that desktop's home; null when none. */
export function readLastPlace(): string | null {
  const store = storage();
  if (!store) return null;
  try {
    const desktopId = store.getItem(LAST_DESKTOP_KEY);
    if (!desktopId) return null;
    const saved = store.getItem(lastPlaceKey(desktopId));
    if (saved && placeDesktop(saved) === desktopId) return saved;
    return `/d/${encodeURIComponent(desktopId)}`;
  } catch {
    return null;
  }
}

/** Forgets what is remembered for a desktop (for example after it is removed). */
export function forgetDesktop(desktopId: string): void {
  const store = storage();
  if (!store) return;
  try {
    store.removeItem(lastPlaceKey(desktopId));
    if (store.getItem(LAST_DESKTOP_KEY) === desktopId) store.removeItem(LAST_DESKTOP_KEY);
  } catch {
    // Nothing to forget.
  }
}

/** Remembers the current location whenever it is a workspace place. Mount it in every workspace screen. */
export function useRememberPlace(): void {
  const href = useLocation({ select: (location) => location.href });
  useEffect(() => {
    rememberPlace(href);
  }, [href]);
}
