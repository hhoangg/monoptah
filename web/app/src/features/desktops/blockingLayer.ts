// Whether a blocking link-state layer is on screen for a desktop. Owner: WS-M (W-04).
// The layer reports here and the switcher bar reads it, so the bar is never rendered while any layer
// (including the delayed "Requesting access..." one) shows.
import { useSyncExternalStore } from "react";

const shown = new Set<string>();
const listeners = new Set<() => void>();

export function setBlockingLayerShown(desktopId: string, value: boolean): void {
  if (shown.has(desktopId) === value) return;
  if (value) shown.add(desktopId);
  else shown.delete(desktopId);
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useBlockingLayerShown(desktopId: string): boolean {
  return useSyncExternalStore(
    subscribe,
    () => shown.has(desktopId),
    () => false,
  );
}
