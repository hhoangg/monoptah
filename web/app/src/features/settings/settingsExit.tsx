// How a person leaves the settings shell. The shell has no app header (no brand link), so a button before the "Settings"
// title is the way out, on the wide nav and on the narrow list page.
import { onlineManager, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useNavigate, useRouter, useRouterState } from "@tanstack/react-router";
import { useCallback, useEffect, useRef } from "react";
import { desktopsListQuery, type DesktopSummary } from "@/features/desktops/api";
import { readLastUsed, sortByLastUsed } from "@/features/desktops/storage";
import { placeDesktop, readLastPlace } from "@/features/sessions/lastPlace";
import { cn } from "@/lib/utils";
import { Icon } from "@/ui/Icon";
import { currentHistoryIndex, entryBeforeSettings } from "./historyLog";

/**
 * Returns the function that leaves settings.
 *
 * It does not step back one entry: the person may have opened several settings pages since, and "back" from the exit
 * button must leave settings, not walk through them. The per-tab history log (historyLog.ts) knows the path of every
 * entry this tab visited, so the button goes to the nearest earlier entry that is neither a settings page nor a sign-in
 * page, with one `go` of a negative delta.
 *
 * With no such entry known (settings opened directly, a fresh tab) it replaces the settings entry, so the exit never grows
 * the history, and picks the place itself instead of going to `/` first: `/` has no last place for an account that never
 * opened a desktop and redirects to /settings/desktops, which would loop back into settings. The list is the cached one
 * when there is one (the nav polls it, and offline it is the saved snapshot), else it is loaded quickly (it gives up after about 3 seconds, and at once offline). In order: no desktop in the account -> `/pair` (the Pair a desktop page); the last place's desktop while it is
 * in the list; else the most recently used desktop of the list. When the list is unknown: the last place's desktop, else
 * `/pair`. The navigation is dropped when the location changed while the list loaded, and a second click is ignored until
 * the location changes (a second `go` could overshoot).
 */
export function useSettingsExit(): () => void {
  const router = useRouter();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  // The location a navigation has committed, not the one it started: the shell stays clickable until the match commits.
  const resolvedHref = useRouterState({ select: (state) => state.resolvedLocation?.href });
  // The token of the call that holds the latch, or null when the button is free.
  const leaving = useRef<symbol | null>(null);
  useEffect(() => {
    leaving.current = null;
  }, [resolvedHref]);
  return useCallback(() => {
    if (leaving.current !== null) return;
    const token = Symbol("leave");
    leaving.current = token;
    const current = currentHistoryIndex(router);
    const target = current === null ? null : entryBeforeSettings(current);
    if (current !== null && target !== null && target - current < 0) {
      // Stays latched until the resolved location changes.
      router.history.go(target - current);
      return;
    }
    const startHref = router.state.location.href;
    void leaveWithoutHistory(navigate, queryClient, () => router.state.location.href === startHref).finally(() => {
      // Back to clickable once the fallback is over, unless a newer call holds the latch.
      if (leaving.current === token) leaving.current = null;
    });
  }, [router, navigate, queryClient]);
}

/** How long the exit waits for the desktops list before treating it as unknown. */
const LOAD_TIMEOUT_MS = 3000;

/**
 * The account's desktops: the cached list when there is one; null (unknown) at once offline or while the list is failing;
 * else a fetch that gives up after about 3 seconds. A shared in-flight request (the nav's, with retries) cannot block it.
 */
async function loadDesktops(queryClient: QueryClient): Promise<DesktopSummary[] | null> {
  const cached = queryClient.getQueryData(desktopsListQuery.queryKey);
  if (cached !== undefined) return cached.desktops;
  if ((typeof navigator !== "undefined" && navigator.onLine === false) || !onlineManager.isOnline()) return null;
  const state = queryClient.getQueryState(desktopsListQuery.queryKey);
  if (state !== undefined && (state.fetchFailureCount > 0 || state.status === "error")) return null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), LOAD_TIMEOUT_MS);
  });
  const load = queryClient
    .fetchQuery({ ...desktopsListQuery, staleTime: 30_000, retry: false, networkMode: "always" })
    .then((result) => result.desktops)
    .catch(() => null);
  try {
    return await Promise.race([load, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

async function leaveWithoutHistory(navigate: ReturnType<typeof useNavigate>, queryClient: QueryClient, stillHere: () => boolean): Promise<void> {
  const place = readLastPlace();
  const placeId = place === null ? null : placeDesktop(place);
  const desktops = await loadDesktops(queryClient);
  if (!stillHere()) return;
  const open = (desktopId: string) => navigate({ to: "/d/$desktopId", params: { desktopId }, replace: true });
  if (desktops === null) {
    await (placeId !== null ? open(placeId) : navigate({ to: "/pair", replace: true }));
    return;
  }
  // A desktop that left the list (revoked, removed) is not worth opening.
  const target = placeId !== null && desktops.some((desktop) => desktop.id === placeId) ? placeId : sortByLastUsed(desktops, readLastUsed())[0]?.id;
  await (desktops.length === 0 || target === undefined ? navigate({ to: "/pair", replace: true }) : open(target));
}

/** The 44px circle before the "Settings" title that leaves the settings shell (see `useSettingsExit`). */
export function SettingsExitButton({ className }: { className?: string }) {
  const exit = useSettingsExit();
  return (
    <button
      type="button"
      aria-label="Leave settings"
      onClick={exit}
      className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection", className)}
    >
      <Icon name="chevronDown" size={20} className="rotate-90" />
    </button>
  );
}
