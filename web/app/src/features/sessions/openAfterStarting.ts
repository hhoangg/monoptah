// F-20: whether this browser opens a newly started session (ported from
// mobile/src/features/settings/newSessions.ts). On until turned off. The new-session page reads it with
// `useOpenAfterStarting()`; a settings row can flip it with the same hook.
import { useCallback, useSyncExternalStore } from "react";

export const OPEN_AFTER_STARTING_KEY = "monoptah.newSessions.openAfterStarting";

const listeners = new Set<() => void>();

/** Reads the stored choice; anything but an explicit "false" means on. */
export function readOpenAfterStarting(): boolean {
  try {
    return window.localStorage.getItem(OPEN_AFTER_STARTING_KEY) !== "false";
  } catch {
    return true;
  }
}

/** Set when storage refused the write, so the choice still lasts until the page reloads. */
let unsaved: boolean | null = null;

export function writeOpenAfterStarting(value: boolean): void {
  unsaved = null;
  try {
    window.localStorage.setItem(OPEN_AFTER_STARTING_KEY, String(value));
  } catch {
    unsaved = value;
  }
  listeners.forEach((listener) => listener());
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key === OPEN_AFTER_STARTING_KEY || event.key === null) listener();
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
};

/** `[value, set]`: true by default; the choice survives a reload and follows other tabs. */
export function useOpenAfterStarting(): readonly [boolean, (value: boolean) => void] {
  const value = useSyncExternalStore(subscribe, () => unsaved ?? readOpenAfterStarting(), () => true);
  const set = useCallback((next: boolean) => writeOpenAfterStarting(next), []);
  return [value, set] as const;
}
