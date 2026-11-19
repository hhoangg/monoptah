// Mounts the link connection of one desktop (architecture.md 3.3): connects on mount and releases on
// unmount through a ref count, so two providers for the same desktop share one connection.
import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { getLinkConnection, type LinkConnection } from "./connection.ts";
import { requestPersistentStorage } from "./keys.ts";
import { useLink, type LinkState } from "./store.ts";

interface LinkContextValue {
  desktopId: string;
  connection: LinkConnection;
}

const LinkContext = createContext<LinkContextValue | null>(null);

let persistentStorageRequested = false;

export function LinkProvider({ desktopId, children }: { desktopId: string; children: ReactNode }) {
  const connection = useMemo(() => getLinkConnection(desktopId), [desktopId]);
  const value = useMemo(() => ({ desktopId, connection }), [desktopId, connection]);

  useEffect(() => {
    connection.retain();
    return () => connection.release();
  }, [connection]);

  useEffect(() => {
    if (persistentStorageRequested) return;
    persistentStorageRequested = true;
    void requestPersistentStorage();
  }, []);

  return <LinkContext.Provider value={value}>{children}</LinkContext.Provider>;
}

function useLinkContext(): LinkContextValue {
  const value = useContext(LinkContext);
  if (value === null) throw new Error("useLinkDesktopId must be used inside a LinkProvider");
  return value;
}

/** The desktop id of the nearest `LinkProvider`. Throws outside a `/d/$desktopId` route. */
export function useLinkDesktopId(): string {
  return useLinkContext().desktopId;
}

/** The connection of the nearest `LinkProvider`, for requests, watches and `onResynced`. */
export function useLinkConnection(): LinkConnection {
  return useLinkContext().connection;
}

/** The connection state of the nearest `LinkProvider`'s desktop; optionally a selected slice. */
export function useLinkState(): LinkState;
export function useLinkState<T>(selector: (state: LinkState) => T): T;
export function useLinkState<T>(selector?: (state: LinkState) => T): T | LinkState {
  const desktopId = useLinkDesktopId();
  return useLink(desktopId, (selector ?? ((state: LinkState) => state)) as (state: LinkState) => T | LinkState);
}
