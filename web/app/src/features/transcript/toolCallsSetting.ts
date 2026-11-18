// Ported from mobile/src/features/settings/toolCalls.ts: whether work phases show only their label until
// opened, even the one in progress. On until turned off. There is no settings control for it yet
// (WS-S: see the Task 4 report); the value lives under `monoptah.toolCalls.collapsed`.
import { useSyncExternalStore } from "react";

export const TOOL_CALLS_KEY = "monoptah.toolCalls.collapsed";

function load(): boolean {
  try {
    return window.localStorage.getItem(TOOL_CALLS_KEY) !== "false";
  } catch {
    return true;
  }
}

let collapsed: boolean | null = null;
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

export function setCollapsedToolCalls(value: boolean): void {
  collapsed = value;
  try {
    window.localStorage.setItem(TOOL_CALLS_KEY, String(value));
  } catch {
    // Storage blocked: the choice holds until reload.
  }
  for (const listener of listeners) listener();
}

/** Test hook: forget the held value so the next read loads from storage. */
export function resetCollapsedToolCalls(): void {
  collapsed = null;
}

export function useCollapsedToolCalls(): [boolean, (value: boolean) => void] {
  const value = useSyncExternalStore(subscribe, () => (collapsed ??= load()), () => true);
  return [value, setCollapsedToolCalls];
}
