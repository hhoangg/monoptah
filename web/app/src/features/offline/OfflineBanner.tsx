// Global connection banner (F-07, N-17; the composer uses `useConnectivity` for F-42):
// "You are offline", "Desktop offline" with the last time the desktop was heard from, "Reconnecting...".
// An overlay toast (no layout shift) behind a short show delay. A polite live region that is always mounted, so a
// change is announced. Glass surface, tokens only.
import { AnimatePresence, useReducedMotion } from "motion/react";
import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { desktopsQuery } from "@/features/accounts/queries";
import { getLinkConnection, useLink } from "@/link";
import { cn } from "@/lib/utils";
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
  // The directory knows when the desktop was last seen, which this tab cannot when the desktop was offline at open.
  const directory = useQuery({ ...desktopsQuery, enabled: desktopId !== null && online && presence === "offline" && lastFrameAt === null });
  // The directory counts seconds (the Worker's `last_seen_at`); the link and the banner use milliseconds.
  const directorySeconds = directory.data?.desktops.find((desktop) => desktop.id === desktopId)?.lastSeenAt ?? null;
  const directoryLastSeenAt = directorySeconds === null ? null : directorySeconds * 1000;
  return describeConnectivity(online, desktopId === null ? null : { status, presence, blocked, lastFrameAt }, directoryLastSeenAt);
}

/** A short-lived blip (a quick reconnect after the PWA resumes) never reaches the screen. */
export const SHOW_DELAY_MS = 400;

/** True once `active` has held for `delay` ms; false again the moment it ends. */
function useShowDelay(active: boolean, delay: number): boolean {
  const [elapsed, setElapsed] = useState(false);
  useEffect(() => {
    if (!active) return;
    const timer = window.setTimeout(() => setElapsed(true), delay);
    return () => {
      window.clearTimeout(timer);
      setElapsed(false);
    };
  }, [active, delay]);
  return active && elapsed;
}

/** Pinned banners that currently show; the document attribute stays while at least one does (several banners, StrictMode double effects). */
let markOwners = 0;

/** Marks the document for the settings anchors; returns the release function. The attribute goes only with the last owner. */
function claimConnectionBannerMark(): () => void {
  const root = document.documentElement;
  markOwners += 1;
  root.setAttribute("data-connection-banner", "shown");
  let released = false;
  return () => {
    if (released) return;
    released = true;
    markOwners -= 1;
    if (markOwners === 0) root.removeAttribute("data-connection-banner");
  };
}

/** Publishes the pill's height as `--connection-banner-height` while it shows; the variable goes with the last owner. */
let heightOwners = 0;

/** Publishes the measured height of `element`; returns the release function. A no-op measure where ResizeObserver is missing. */
function claimConnectionBannerHeight(element: HTMLElement): () => void {
  const root = document.documentElement;
  heightOwners += 1;
  const publish = () => root.style.setProperty("--connection-banner-height", `${element.getBoundingClientRect().height}px`);
  publish();
  const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(publish);
  observer?.observe(element);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    observer?.disconnect();
    heightOwners -= 1;
    if (heightOwners === 0) root.style.removeProperty("--connection-banner-height");
  };
}

/**
 * The connection status as an overlay toast. The live region is a zero-height box in the flow (right under the top
 * bar, which already clears the notch), so showing or hiding it never moves the content; the pill floats over the
 * content from there. Only the pill takes pointer events. It is the single live region for the connection state
 * (the pill also says the saved data is shown).
 */
export function OfflineBanner({
  pinnedBelowHeader = false,
  pinned = pinnedBelowHeader ? "below-header" : undefined,
}: {
  /** Same as `pinned="below-header"`. Only tests pass it; new callers use `pinned`. */
  pinnedBelowHeader?: boolean;
  /**
   * The page scrolls (not a /d frame): stay in view. `below-header` sits just under the sticky 44px app header; `top` is
   * for a page without that header (settings), where only the notch inset is above the banner.
   */
  pinned?: "below-header" | "top";
}) {
  const desktopId = useRouterState({ select: (state) => desktopIdFromPath(state.location.pathname) });
  const connectivity = useDesktopConnectivity(desktopId);
  const reduceMotion = useReducedMotion();
  const showing = useShowDelay(connectivity.kind !== "online", SHOW_DELAY_MS);
  const canRetry = desktopId !== null && ((connectivity.kind === "desktop_offline" || (connectivity.kind === "reconnecting" && connectivity.connecting !== true)));
  const offset = reduceMotion ? 0 : -8;
  // Only a pinned pill floats over a scrolling page: tell the settings anchors (`--settings-anchor-offset`) to clear it.
  const covering = pinned === "top" && showing;
  useEffect(() => {
    if (!covering) return;
    return claimConnectionBannerMark();
  }, [covering]);
  const [pill, setPill] = useState<HTMLElement | null>(null);
  const measured = covering ? pill : null;
  useEffect(() => {
    if (measured === null) return;
    return claimConnectionBannerHeight(measured);
  }, [measured]);
  return (
    <div role="status" aria-live="polite" data-testid="connection-banner" className={cn("pointer-events-none z-20 h-0 shrink-0", pinned === "below-header" ? "sticky top-[calc(env(safe-area-inset-top)+2.75rem)]" : pinned === "top" ? "sticky top-[env(safe-area-inset-top)]" : "relative")}>
      <AnimatePresence>
        {showing ? (
          <div key="toast" className="absolute inset-x-0 top-2 flex justify-center pr-[max(0.5rem,env(safe-area-inset-right))] pl-[max(0.5rem,env(safe-area-inset-left))]">
            <Glass
              ref={setPill}
              data-kind={connectivity.kind}
              initial={{ opacity: 0, y: offset }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: offset }}
              transition={{ duration: reduceMotion ? 0 : 0.2, ease: "easeOut" }}
              className="pointer-events-auto flex min-h-9 max-w-full items-center gap-2 rounded-[20px] bg-[var(--sheet)] px-3 py-1.5 text-small text-content"
            >
              <Icon name={connectivity.kind === "reconnecting" ? "spinner" : "computerOff"} size={18} className="shrink-0 text-muted-text" />
              <span className="min-w-0">
                <span className="font-semibold">{connectivity.label}</span>
                {connectivity.lastSeenAt !== null ? <span className="text-muted-text">{` - last seen ${formatLastSeen(connectivity.lastSeenAt)}`}</span> : null}
                <span className="text-muted-text">{" - showing saved data"}</span>
              </span>
              {canRetry && desktopId !== null ? (
                <Button variant="ghost" size="sm" onClick={() => getLinkConnection(desktopId).reconnect()}>
                  Retry
                </Button>
              ) : null}
            </Glass>
          </div>
        ) : null}
      </AnimatePresence>
    </div>
  );
}
