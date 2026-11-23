// A per-tab log of the path of every history entry this tab has visited, keyed by the entry's index
// (`history.location.state.__TSR_index`). It lives in sessionStorage so a reload or a Back/Forward re-entry still knows
// where the person came from. The settings exit button reads it to find the entry before settings was entered.
import type { AnyRouter } from "@tanstack/react-router";

const KEY = "monoptah.historyLog";

/**
 * Pages the exit never lands on: sign-in pages of a visitor who has no session (the sign-in flow would bounce back) and
 * one-shot pages (`/pair/CODE` shows an expired screen once the code was used, and `/pair` is where the exit sends an account with no desktop, so leaving settings after pairing must not return to it). Entries match by exact path or prefix.
 */
const SKIPPED_PATHS: readonly string[] = ["/login", "/register", "/recover", "/pair", "/pair/"];

type Log = (string | null)[];

function store(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function readLog(): Log {
  try {
    const parsed: unknown = JSON.parse(store()?.getItem(KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.map((item) => (typeof item === "string" ? item : null)) : [];
  } catch {
    return [];
  }
}

function normalize(pathname: string): string {
  return pathname.replace(/(.)\/$/, "$1");
}

/** The index of the current history entry, or null when the history does not number its entries. */
export function currentHistoryIndex(router: AnyRouter): number | null {
  const index = router.history.location.state?.__TSR_index;
  return typeof index === "number" ? index : null;
}

/**
 * Records the current entry. Nothing is truncated: pushes are sequential, so every lower entry reachable from the current
 * one was logged, and a stale entry above the index is never read (a push overwrites it). A Back/Go jump therefore keeps
 * the entries it passed over.
 */
export function recordHistoryEntry(index: number, pathname: string): void {
  const storage = store();
  if (!storage) return;
  try {
    const log = readLog();
    const path = normalize(pathname);
    if (log[index] === path) return;
    for (let i = log.length; i < index; i += 1) log[i] = null;
    log[index] = path;
    storage.setItem(KEY, JSON.stringify(log));
  } catch {
    // Storage full or blocked: the exit falls back to the last used desktop.
  }
}

/** Logs every resolved navigation of the router. Returns the unsubscribe function. */
export function trackHistory(router: AnyRouter): () => void {
  return router.subscribe("onResolved", () => {
    const index = currentHistoryIndex(router);
    if (index !== null) recordHistoryEntry(index, router.state.location.pathname);
  });
}

function isSkippedPath(path: string): boolean {
  return SKIPPED_PATHS.some((skipped) => (skipped.endsWith("/") ? path.startsWith(skipped) : path === skipped));
}

function isSettingsPath(path: string): boolean {
  return path === "/settings" || path.startsWith("/settings/");
}

/**
 * The index of the nearest entry below `index` that is neither a settings page nor a skipped page, or null when none is
 * known (a direct open, a reload of a fresh tab). An entry that was never logged is unknown, so the search stops there
 * instead of skipping past it and landing on the wrong page.
 */
export function entryBeforeSettings(index: number): number | null {
  const log = readLog();
  for (let i = index - 1; i >= 0; i -= 1) {
    const path = log[i];
    if (typeof path !== "string") return null;
    if (!isSettingsPath(path) && !isSkippedPath(path)) return i;
  }
  return null;
}
