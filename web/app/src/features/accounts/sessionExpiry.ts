// One place that reacts to a signed-out session (security review L2): when any session-auth call
// (a query or an action, both go through apiFetch) fails with 401, the cache is dropped and the
// user is sent to /login with the page they were on. Public auth routes are not watched: a 401 there
// is a normal failure. Screens do not handle 401 themselves. The root auth guard still handles its own `me` call during
// a navigation; this handler skips that case (the router is already resolving a location).
import type { QueryClient } from "@tanstack/react-query";
import { onSessionExpired } from "./api";
import { isPublicPath } from "./authGuard";
import { clearAccountCache } from "./queries";
import { loginRedirectTarget, saveFingerprintFragment } from "./redirect";

/** The few router members this module needs; the real `router` satisfies it. */
export interface ExpiryRouter {
  state: { isLoading: boolean; location: { pathname: string; searchStr: string; hash: string } };
  navigate: (options: { to: "/login"; search: { redirect: string }; replace: true }) => unknown;
}

/** Returns the unsubscribe function. */
export function installSessionExpiryHandler(queryClient: QueryClient, router: ExpiryRouter): () => void {
  let leaving = false;
  return onSessionExpired(() => {
    const { pathname, searchStr, hash } = router.state.location;
    if (leaving || router.state.isLoading || isPublicPath(pathname)) return;
    leaving = true;
    // Same as the guard: a `#fp` on /pair or /verify survives the sign-in.
    saveFingerprintFragment(pathname, hash);
    // Clear after the failing call has finished reporting.
    queueMicrotask(() => {
      clearAccountCache(queryClient);
      void Promise.resolve(router.navigate({ to: "/login", search: { redirect: loginRedirectTarget(pathname, searchStr) }, replace: true })).finally(() => {
        leaving = false;
      });
    });
  });
}
