// The per-desktop store of the session tabs and panes, kept in localStorage as `monoptah.layout.tabs.<desktopId>`
// (a Workspace of paneTree.ts, wrapped as {v: 1, ws}). The text in localStorage is the source of truth: a snapshot is
// parsed again only when the stored text changed, so clearing storage (tests, another tab of the browser signing out)
// starts from an empty workspace. A blocked or full storage keeps the workspace in memory for this page. Another
// browser tab writing the same desktop's workspace fires a `storage` event here: this page adopts that state at once,
// so two tabs of one desktop do not overwrite each other at some later render. What it adopts is the tab sets and the
// pane tree: which pane is focused and which tab is active stay what this page had, because they follow this page's URL
// (another browser tab opening a session must not change the chat shown here). A merge writes nothing, so two browser
// tabs cannot ping-pong.
import { useSyncExternalStore } from "react";
import { parseWorkspace, serializeWorkspace, type Workspace } from "./paneTree";

/** `incoming` (written by another browser tab) with the focused pane and the active tabs of `current` where they are still valid. */
function keepLocalView(current: Workspace, incoming: Workspace): Workspace {
  const panes = { ...incoming.panes };
  for (const [id, pane] of Object.entries(incoming.panes)) {
    const mine = current.panes[id]?.active;
    if (mine && pane.tabs.includes(mine)) panes[id] = { ...pane, active: mine };
  }
  return { ...incoming, panes, focused: incoming.panes[current.focused] ? current.focused : incoming.focused };
}

export const tabsKey = (desktopId: string) => `monoptah.layout.tabs.${desktopId}`;

interface Entry {
  raw: string | null;
  ws: Workspace;
  /** False when the last write failed: the in-memory workspace wins over an unchanged (or missing) stored one. */
  persisted: boolean;
}

const cache = new Map<string, Entry>();
const listeners = new Set<() => void>();

function readRaw(desktopId: string): string | null {
  try {
    return window.localStorage.getItem(tabsKey(desktopId));
  } catch {
    return null;
  }
}

/** The current workspace of a desktop. The same object until it changes. */
export function getWorkspace(desktopId: string): Workspace {
  const raw = readRaw(desktopId);
  const entry = cache.get(desktopId);
  if (entry && (entry.raw === raw || !entry.persisted)) return entry.ws;
  const ws = parseWorkspace(raw);
  cache.set(desktopId, { raw, ws, persisted: true });
  return ws;
}

/** Stores the result of `change(current)`; a result that is the same object is ignored. `persist: false` keeps it in memory (a divider drag in progress: the commit writes it). */
export function updateWorkspace(desktopId: string, change: (ws: Workspace) => Workspace, options: { persist?: boolean } = {}): void {
  const current = getWorkspace(desktopId);
  const next = change(current);
  if (next === current) return;
  const raw = serializeWorkspace(next);
  let stored = false;
  if (options.persist !== false) {
    try {
      window.localStorage.setItem(tabsKey(desktopId), raw);
      stored = true;
    } catch {
      // storage blocked or full: the workspace stays in memory
    }
  }
  cache.set(desktopId, { raw, ws: next, persisted: stored });
  for (const listener of [...listeners]) listener();
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    // `key` is null when the whole storage was cleared.
    if (event.key !== null && !event.key.startsWith(tabsKey(""))) return;
    for (const [desktopId, entry] of [...cache]) {
      const raw = readRaw(desktopId);
      if (raw === entry.raw || raw === null) continue;
      cache.set(desktopId, { raw, ws: keepLocalView(entry.ws, parseWorkspace(raw)), persisted: true });
    }
    for (const listener of [...listeners]) listener();
  });
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useWorkspace(desktopId: string): Workspace {
  return useSyncExternalStore(subscribe, () => getWorkspace(desktopId), () => getWorkspace(desktopId));
}

/** Forgets the in-memory snapshots (for tests, after clearing localStorage). */
export function resetTabsStore(): void {
  cache.clear();
  for (const listener of [...listeners]) listener();
}
