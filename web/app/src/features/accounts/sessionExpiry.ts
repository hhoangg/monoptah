// One place that reacts to a signed-out session (security review L2): when any query fails with
// 401, the account cache is dropped and the user is sent to /login with the page they were on.
// Screens do not handle 401 themselves. The root auth guard still handles its own `me` call during
// a navigation; this handler skips that case (the router is already resolving a location).
import type { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./api";
import { isPublicPath } from "./authGuard";
import { clearAccountCache } from "./queries";
import { loginRedirectTarget, saveFingerprintFragment } from "./redirect";

/** The few router members this module needs; the real `router` satisfies it. */
export interface ExpiryRouter {
  state: { isLoading: boolean; location: { pathname: string; searchStr: string; hash: string } };
  navigate: (options: { to: "/login"; search: { redirect: string }; replace: true }) => unknown;
}

/** Watches the query cache and the mutation cache; returns one unsubscribe function. */
export function installSessionExpiryHandler(queryClient: QueryClient, router: ExpiryRouter): () => void {
  let leaving = false;
  function onUnauthorized(error: unknown): void {
    if (!(error instanceof ApiError) || error.status !== 401) return;
    const { pathname, searchStr, hash } = router.state.location;
    if (leaving || router.state.isLoading || isPublicPath(pathname)) return;
    leaving = true;
    // Same as the guard: a `#fp` on /pair or /verify survives the sign-in.
    saveFingerprintFragment(pathname, hash);
    // Clear after the cache finishes notifying about this failure.
    queueMicrotask(() => {
      clearAccountCache(queryClient);
      void Promise.resolve(router.navigate({ to: "/login", search: { redirect: loginRedirectTarget(pathname, searchStr) }, replace: true })).finally(() => {
        leaving = false;
      });
    });
  }
  const offQueries = queryClient.getQueryCache().subscribe((event) => {
    if (event.type === "updated" && event.action.type === "error") onUnauthorized(event.action.error);
  });
  const offMutations = queryClient.getMutationCache().subscribe((event) => {
    if (event.type === "updated" && event.action.type === "error") onUnauthorized(event.action.error);
  });
  return () => {
    offQueries();
    offMutations();
  };
}
