// Global connection banner (F-07, N-17; the composer and transcript use `useConnectivity` for F-42):
// "You are offline", "Desktop offline" with the last time the desktop was heard from, "Reconnecting...".
// A polite live region that is always mounted, so a change is announced. Glass surface, tokens only.
import { useRouterState } from "@tanstack/react-router";
import { getLinkConnection, useLink } from "@/link";
import { Button } from "@/ui/button";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { describeConnectivity, formatLastSeen, useOnline, type Connectivity } from "./connectivity";

const DESKTOP_PATH = /^\/d\/([^/]+)/;

/** The desktop id of the route being shown, or null outside `/d/:desktopId`. */
export function desktopIdFromPath(pathname: string): string | null {
  const match = DESKTOP_PATH.exec(pathname);
  if (!match?.[1]) return null;
  try {
    return decodeURIComponent(match[1]);
  } catch {
    return null;
  }
}

/** Connectivity for the current route: the browser state plus the link state of the open desktop. */
export function useConnectivity(): Connectivity {
  const desktopId = useRouterState({ select: (state) => desktopIdFromPath(state.location.pathname) });
  return useDesktopConnectivity(desktopId);
}

/** The same for a given desktop (null: browser state only). */
export function useDesktopConnectivity(desktopId: string | null): Connectivity {
  const online = useOnline();
  const status = useLink(desktopId ?? "", (state) => state.status);
  const presence = useLink(desktopId ?? "", (state) => state.presence);
  const blocked = useLink(desktopId ?? "", (state) => state.blocked);
  const lastFrameAt = useLink(desktopId ?? "", (state) => state.lastFrameAt);
  return describeConnectivity(online, desktopId === null ? null : { status, presence, blocked, lastFrameAt });
}

export function OfflineBanner() {
  const desktopId = useRouterState({ select: (state) => desktopIdFromPath(state.location.pathname) });
  const connectivity = useDesktopConnectivity(desktopId);
  const showing = connectivity.kind !== "online";
  const canRetry = desktopId !== null && (connectivity.kind === "reconnecting" || connectivity.kind === "desktop_offline");
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="connection-banner"
      className={showing ? "px-2 pt-2 min-[24rem]:px-5" : undefined}
    >
      {showing ? (
        <Glass
          data-kind={connectivity.kind}
          className="mx-auto flex min-h-11 max-w-3xl items-center gap-2 rounded-[16px] px-3 py-2 text-small text-content"
        >
          <Icon name={connectivity.kind === "reconnecting" ? "spinner" : "computerOff"} size={18} className="shrink-0 text-muted-text" />
          <span className="min-w-0 flex-1">
            <span className="font-semibold">{connectivity.label}</span>
            {connectivity.lastSeenAt !== null ? <span className="text-muted-text">{` - last seen ${formatLastSeen(connectivity.lastSeenAt)}`}</span> : null}
            {connectivity.kind === "offline" ? <span className="block text-muted-text">Saved data stays readable. Sending waits for the connection.</span> : null}
          </span>
          {canRetry && desktopId !== null ? (
            <Button variant="ghost" size="sm" onClick={() => getLinkConnection(desktopId).reconnect()}>
              Retry
            </Button>
          ) : null}
        </Glass>
      ) : null}
    </div>
  );
}
