// Link connection state per desktop (architecture.md 3.2). Public information only: no keys, no
// plaintext payloads, no request or response bodies.
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

export const LINK_STATUSES = ["idle", "connecting", "handshaking", "pending", "ready", "reconnecting", "closed"] as const;
export type LinkStatus = (typeof LINK_STATUSES)[number];

export const LINK_BLOCKED_REASONS = [
  "revoked",
  "account_blocked",
  "key_changed",
  "login_required",
  "policy",
  /** The desktop speaks a newer protocol, or refused an older web prologue: "Reload to update the web app". */
  "web_outdated",
  /** The desktop speaks an older protocol than this web app: "Update Monoptah on your desktop". */
  "desktop_outdated",
  /** BYE before any `state`: unauthenticated, so not proof of a revocation. Copy for WS-M: "This desktop did not accept this device". */
  "desktop_refused",
] as const;
export type LinkBlockedReason = (typeof LINK_BLOCKED_REASONS)[number];

/** What went wrong last. `code` is a stable key: a relay error code, `close_<code>`, a `LinkEndReason`, or `directory_<status>`. */
export interface LinkErrorInfo {
  code: string;
  closeCode?: number;
}

/** An open watch: the registration id and the method only (the params stay with the data layer). */
export interface LinkWatchInfo {
  id: number;
  method: string;
}

export interface LinkState {
  status: LinkStatus;
  permission: "read" | "control" | null;
  desktopName: string | null;
  desktopVersion: string | null;
  proto: number | null;
  capabilities: string[];
  /** The relay's view of the desktop socket; null until known. */
  presence: "online" | "offline" | null;
  /** Epoch ms of the last frame on this tab's channel, refreshed at most every 5 s (`connection.lastFrameAt` is exact). */
  lastFrameAt: number | null;
  watches: LinkWatchInfo[];
  revoked: boolean;
  blocked: LinkBlockedReason | null;
  /** Public fingerprints (lowercase hex) for the "Confirm this device on the desktop" screen; the desktop one is the PINNED key's. */
  deviceFingerprint: string | null;
  desktopFingerprint: string | null;
  lastError: LinkErrorInfo | null;
  /** Epoch ms of the next reconnect attempt while one is scheduled. */
  nextAttemptAt: number | null;
}

export function initialLinkState(): LinkState {
  return {
    status: "idle",
    permission: null,
    desktopName: null,
    desktopVersion: null,
    proto: null,
    capabilities: [],
    presence: null,
    lastFrameAt: null,
    watches: [],
    revoked: false,
    blocked: null,
    deviceFingerprint: null,
    desktopFingerprint: null,
    lastError: null,
    nextAttemptAt: null,
  };
}

const stores = new Map<string, StoreApi<LinkState>>();

/** The store of one desktop (created on first use). For non-React code. */
export function getLinkStore(desktopId: string): StoreApi<LinkState> {
  let store = stores.get(desktopId);
  if (!store) {
    store = createStore<LinkState>(() => initialLinkState());
    stores.set(desktopId, store);
  }
  return store;
}

/** Non-React accessor for the current state of one desktop. */
export function getLinkState(desktopId: string): LinkState {
  return getLinkStore(desktopId).getState();
}

/** Selector hook: `useLink(desktopId, (s) => s.status)`; without a selector it returns the whole state. */
export function useLink(desktopId: string): LinkState;
export function useLink<T>(desktopId: string, selector: (state: LinkState) => T): T;
export function useLink<T>(desktopId: string, selector?: (state: LinkState) => T): T | LinkState {
  return useStore(getLinkStore(desktopId), (selector ?? ((state: LinkState) => state)) as (state: LinkState) => T | LinkState);
}

/** Back to the initial state (a new connect run, or tests). */
export function resetLinkStore(desktopId: string): void {
  getLinkStore(desktopId).setState(initialLinkState(), true);
}

/** Drops every store. For tests. */
export function clearLinkStores(): void {
  stores.clear();
}
