// Wave 0 placeholder of the link provider (architecture.md 3.3). WS-L (R-10, W-05) replaces it with
// the real connection lifecycle; for now it only provides the desktop id of the `/d/$desktopId` layout.
import { createContext, useContext, type ReactNode } from "react";

const LinkDesktopIdContext = createContext<string | null>(null);

export function LinkProvider({ desktopId, children }: { desktopId: string; children: ReactNode }) {
  return <LinkDesktopIdContext.Provider value={desktopId}>{children}</LinkDesktopIdContext.Provider>;
}

/** The desktop id of the nearest `LinkProvider`. Throws outside a `/d/$desktopId` route. */
export function useLinkDesktopId(): string {
  const id = useContext(LinkDesktopIdContext);
  if (id === null) throw new Error("useLinkDesktopId must be used inside a LinkProvider");
  return id;
}
