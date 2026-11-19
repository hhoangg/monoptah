// The tab being dragged (HTML5 drag and drop). Browsers hide the payload of a drag until the drop, so the strips and
// the pane drop zones read it from here while the pointer moves.
import { useSyncExternalStore } from "react";
import type { TabKey } from "./paneTree";

export interface TabDrag {
  desktopId: string;
  paneId: string;
  key: TabKey;
}

export const TAB_DRAG_TYPE = "application/x-monoptah-tab";

let current: TabDrag | null = null;
const listeners = new Set<() => void>();

export function startTabDrag(drag: TabDrag): void {
  current = drag;
  for (const listener of [...listeners]) listener();
}

export function endTabDrag(): void {
  if (!current) return;
  current = null;
  for (const listener of [...listeners]) listener();
}

export function getTabDrag(): TabDrag | null {
  return current;
}

export function useTabDrag(): TabDrag | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => current,
    () => null,
  );
}
