// A navigation to the place the router is already at (same path, same #anchor) is a no-op for the router: no new history
// entry, no location change. Picking a quick-find or palette result that way must still scroll to its anchor, so the
// picker asks the settings shell with `requestSettingsScroll`, and the shell's hash scrolling (`useScrollRequests`) runs again.
import { useSyncExternalStore } from "react";

let count = 0;
const listeners = new Set<() => void>();

/** Asks the settings shell to scroll to the `#anchor` of the current URL again. */
export function requestSettingsScroll(): void {
  count += 1;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Counts the requests so far; an effect that depends on it runs once per request. */
export function useScrollRequests(): number {
  return useSyncExternalStore(subscribe, () => count, () => 0);
}

/** Calls `requestSettingsScroll` when `target` is where `current` already is, so the router will not move. */
export function requestScrollIfAlreadyThere(current: { pathname: string; hash: string }, target: { pathname: string; hash: string | undefined }): void {
  const trim = (path: string) => path.replace(/(.)\/$/, "$1");
  if (target.hash && trim(current.pathname) === trim(target.pathname) && current.hash === target.hash) requestSettingsScroll();
}
