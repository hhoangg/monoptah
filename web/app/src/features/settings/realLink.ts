// Adapter from the real link client (src/link, WS-L) to the `DesktopLink` the settings screens use,
// plus the forget handler that clears what WS-L exposes per desktop (W-11, F-08, F-86).
//
// Registered once at startup by register.ts. In dev with VITE_MOCK_API=1 main.tsx installs the mock
// desktop link afterwards, which replaces this provider.
import type { MethodName, ParamsOf, ResultOf } from "@monoptah/protocol";
import { clearPersistedCache } from "@/data/persist";
import { getLinkConnection, LinkRequestError, type LinkConnection, type LinkState } from "@/link";
import { DesktopLinkError, onForgetDesktop, provideDesktopLink, type DesktopLink, type DesktopLinkStatus } from "./desktopLink";

/** How long `call` and `reconnect` wait for the handshake before giving up. */
export const LINK_WAIT_MS = 15_000;

/** The part of `LinkConnection` the adapter uses (tests pass a fake). */
export type SettingsLinkConnection = Pick<LinkConnection, "request" | "reconnect" | "retain" | "release" | "store" | "getState">;

export function toDesktopLinkStatus(state: Pick<LinkState, "status" | "blocked" | "presence">): DesktopLinkStatus {
  if (state.blocked !== null) return "failed";
  switch (state.status) {
    case "ready":
      return state.presence === "offline" ? "offline" : "connected";
    case "reconnecting":
      return "reconnecting";
    case "closed":
      return "offline";
    default:
      return "connecting";
  }
}

function toError(error: unknown): DesktopLinkError {
  if (error instanceof DesktopLinkError) return error;
  if (error instanceof LinkRequestError) {
    const offline = error.code === "not_ready" || error.code === "session_ended";
    return new DesktopLinkError(error.message, offline ? "desktop_offline" : error.code);
  }
  return new DesktopLinkError(error instanceof Error ? error.message : "The desktop link failed", "desktop_offline");
}

/** Resolves when the link is ready; rejects when it is blocked, closed for good or the wait runs out. */
function waitUntilReady(connection: SettingsLinkConnection, timeoutMs: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => finish(new DesktopLinkError("The desktop did not answer in time", "desktop_offline")), timeoutMs);
    const finish = (error?: DesktopLinkError) => {
      clearTimeout(timer);
      unsubscribe();
      if (error) reject(error);
      else resolve();
    };
    const check = () => {
      const state = connection.getState();
      if (state.blocked !== null) finish(new DesktopLinkError("The desktop link is blocked", state.blocked));
      else if (state.status === "ready") finish();
      else if (state.status === "closed" || state.status === "idle") finish(new DesktopLinkError("The desktop is offline", "desktop_offline"));
    };
    const unsubscribe = connection.store.subscribe(check);
    check();
  });
}

export function createRealDesktopLink(connection: SettingsLinkConnection, waitMs: number = LINK_WAIT_MS): DesktopLink {
  return {
    get status() {
      return toDesktopLinkStatus(connection.getState());
    },
    subscribe(listener) {
      // Holding a reference keeps the connection running while a screen follows its status, so
      // `reconnect` has something to restart.
      connection.retain();
      const unsubscribe = connection.store.subscribe(listener);
      return () => {
        unsubscribe();
        connection.release();
      };
    },
    async call<M extends MethodName>(method: M, params?: ParamsOf<M>): Promise<ResultOf<M>> {
      try {
        if (connection.getState().status !== "ready") await waitUntilReady(connection, waitMs);
        return await connection.request(method, params);
      } catch (error) {
        throw toError(error);
      }
    },
    async reconnect() {
      try {
        connection.reconnect();
        await waitUntilReady(connection, waitMs);
      } catch (error) {
        throw toError(error);
      }
    },
  };
}

/** Wires the real link into the registry and the forget hook. */
export function installRealDesktopLink(): void {
  provideDesktopLink((desktopId) => createRealDesktopLink(getLinkConnection(desktopId)));
  onForgetDesktop(async (desktopId) => {
    // Stops the socket for good; the next visit builds on a fresh start().
    const connection = getLinkConnection(desktopId);
    connection.stop();
    await clearPersistedCache(desktopId);
  });
}
