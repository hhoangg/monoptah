// State of the move dialogs (U-19). A menu item cannot own its dialog: the menu content unmounts as soon
// as the item is selected, which would take the dialog with it. So the item only records the request here,
// and a `MoveDialogHost` mounted next to the card badge or the header chip (both outlive the menu) renders
// the dialog.
//
// A request belongs to exactly one host and dies with it: `openMove` hands the request to the most recently
// mounted host of that desktop and session, and when that host unmounts (the card scrolls out, the route
// changes) the request is dropped. A request therefore never reappears in another host later, and it is
// keyed by desktop and session, so two desktops that share a session id never mix.
import { useSyncExternalStore } from "react";
import type { SessionCard } from "@monoptah/protocol";

export type MoveActionKind = "toCloud" | "toLocal" | "cancel";

export interface MoveRequest {
  kind: MoveActionKind;
  desktopId: string;
  session: SessionCard;
}

interface Snapshot {
  request: MoveRequest | null;
  /** The host that shows `request`. */
  owner: string | null;
  hosts: ReadonlyArray<{ id: string; key: string }>;
}

let snapshot: Snapshot = { request: null, owner: null, hosts: [] };
const listeners = new Set<() => void>();

function set(next: Snapshot) {
  snapshot = next;
  for (const listener of listeners) listener();
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => void listeners.delete(listener);
};
const getSnapshot = () => snapshot;

export const moveHostKey = (desktopId: string, sessionId: string) => `${desktopId}\u0000${sessionId}`;

/** Opens a request on the latest host of its desktop and session. Without a mounted host nothing opens. */
export function openMove(request: MoveRequest): boolean {
  const key = moveHostKey(request.desktopId, request.session.id);
  const owner = [...snapshot.hosts].reverse().find((host) => host.key === key);
  if (!owner) return false;
  set({ ...snapshot, request, owner: owner.id });
  return true;
}

export function closeMove(): void {
  set({ ...snapshot, request: null, owner: null });
}

/** Registers a host (`id` is unique per mounted host); returns the unregister function, which also drops the host's request. */
export function registerHost(id: string, key: string): () => void {
  set({ ...snapshot, hosts: [...snapshot.hosts, { id, key }] });
  return () => {
    const hosts = snapshot.hosts.filter((host) => host.id !== id);
    set(snapshot.owner === id ? { request: null, owner: null, hosts } : { ...snapshot, hosts });
  };
}

export function useMoveSnapshot(): Snapshot {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** For tests. */
export function resetMoveStore(): void {
  snapshot = { request: null, owner: null, hosts: [] };
  for (const listener of listeners) listener();
}
