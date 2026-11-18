// State of the move dialogs (U-19). A menu item cannot own its dialog: the menu content unmounts as soon
// as the item is selected, which would take the dialog with it. So the item only records the request here,
// and a `MoveDialogHost` mounted next to the card badge or the header chip (both outlive the menu) renders
// the dialog. When two hosts of the same session are mounted (a list card and the header), the most
// recently mounted one is the owner, so exactly one dialog opens.
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
  hosts: ReadonlyArray<{ id: string; sessionId: string }>;
}

let snapshot: Snapshot = { request: null, hosts: [] };
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

export function openMove(request: MoveRequest): void {
  set({ ...snapshot, request });
}

export function closeMove(): void {
  set({ ...snapshot, request: null });
}

/** Registers a host (`id` is unique per mounted host) for a session; returns the unregister function. */
export function registerHost(id: string, sessionId: string): () => void {
  set({ ...snapshot, hosts: [...snapshot.hosts, { id, sessionId }] });
  return () => set({ ...snapshot, hosts: snapshot.hosts.filter((host) => host.id !== id) });
}

export function useMoveSnapshot(): Snapshot {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** For tests. */
export function resetMoveStore(): void {
  snapshot = { request: null, hosts: [] };
  for (const listener of listeners) listener();
}
