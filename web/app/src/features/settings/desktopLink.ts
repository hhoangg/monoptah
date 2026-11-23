// The small desktop-link interface the per-desktop settings talk to (W-11).
//
// WS-L (R-10, W-05) owns the real link client. Until it is wired in, the settings screens only know
// this interface: `provideDesktopLink(factory)` is the registry WS-L fills, `onForgetDesktop(handler)`
// is the hook WS-L (keys) and W-12 (persisted cache) fill so "Remove this desktop from this device"
// clears local state. With no provider the screens show a clear "not connected" state.
import { useCallback, useMemo, useSyncExternalStore } from "react";
import type { MethodName, ParamsOf, ResultOf } from "@monoptah/protocol";

/** What the browser knows about its connection to one desktop. */
export type DesktopLinkStatus = "connecting" | "connected" | "reconnecting" | "offline" | "failed";

/** A failed `call` or `reconnect`. `code` is the desktop code (`forbidden`, `not_selected`, ...) or `desktop_offline`, `closed`. */
export class DesktopLinkError extends Error {
  readonly code: string | undefined;
  constructor(message: string, code?: string) {
    super(message);
    this.name = "DesktopLinkError";
    this.code = code;
  }
}

export interface DesktopLink {
  /** One request over the link; rejects with `DesktopLinkError`. Waits for the handshake when it is still running. */
  call<M extends MethodName>(method: M, params?: ParamsOf<M>): Promise<ResultOf<M>>;
  /** Drops the connection and runs the handshake again. Resolves when connected, rejects with `DesktopLinkError`. */
  reconnect(): Promise<void>;
  readonly status: DesktopLinkStatus;
  /** Calls `listener` whenever `status` changes. Returns the unsubscribe function. */
  subscribe(listener: () => void): () => void;
}

export type DesktopLinkFactory = (desktopId: string) => DesktopLink | null;

let factory: DesktopLinkFactory | null = null;
const links = new Map<string, DesktopLink | null>();

/** WS-L fills this once at startup. The factory runs at most once per desktop id (the result is cached); null means "no link". */
export function provideDesktopLink(next: DesktopLinkFactory | null): void {
  factory = next;
  links.clear();
}

/** The link of a desktop, or null when no provider is registered or the provider has none. */
export function getDesktopLink(desktopId: string): DesktopLink | null {
  if (!factory) return null;
  if (!links.has(desktopId)) links.set(desktopId, factory(desktopId));
  return links.get(desktopId) ?? null;
}

/** Drops the cached link of a desktop (after a local forget) so the next ask builds a fresh one. */
export function dropDesktopLink(desktopId: string): void {
  links.delete(desktopId);
}

const NO_UNSUBSCRIBE = (): void => undefined;

/**
 * The link of a desktop and its live status. `link` is null when none is provided. With `enabled: false` it is null and
 * nothing is built or subscribed, so a caller can decide first whether this browser should touch the desktop at all.
 */
export function useDesktopLink(desktopId: string, options?: { enabled?: boolean }): { link: DesktopLink | null; status: DesktopLinkStatus | null } {
  const enabled = options?.enabled ?? true;
  const link = useMemo(() => (enabled ? getDesktopLink(desktopId) : null), [desktopId, enabled]);
  const subscribe = useCallback((listener: () => void) => (link ? link.subscribe(listener) : NO_UNSUBSCRIBE), [link]);
  const status = useSyncExternalStore(subscribe, () => link?.status ?? null);
  return { link, status };
}

// --- Forget hook ---

export type ForgetHandler = (desktopId: string) => void | Promise<void>;

const forgetHandlers = new Set<ForgetHandler>();

/** Registers a handler that clears local state for a desktop (pinned key, wrapped key, cache). Returns the unregister function. */
export function onForgetDesktop(handler: ForgetHandler): () => void {
  forgetHandlers.add(handler);
  return () => {
    forgetHandlers.delete(handler);
  };
}

/** localStorage flag per desktop: the first-run picker was shown once. Holds "1" only, nothing sensitive. */
export const pickerSeenKey = (desktopId: string): string => `monoptah.pickerSeen.${desktopId}`;

/** Runs every registered handler; with none registered this does nothing. Always drops the cached link and the "picker seen" flag. */
export async function forgetDesktopLocally(desktopId: string): Promise<void> {
  for (const handler of [...forgetHandlers]) await handler(desktopId);
  dropDesktopLink(desktopId);
  // Pairing the same desktop again starts with the first-run picker again.
  try {
    window.localStorage.removeItem(pickerSeenKey(desktopId));
  } catch {
    // Storage unavailable: nothing to clear.
  }
}

/** Drops the provider, the cached links and every forget handler. For tests. */
export function resetDesktopLinks(): void {
  factory = null;
  links.clear();
  forgetHandlers.clear();
}
