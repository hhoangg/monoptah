// Root `beforeLoad` auth guard (architecture.md 3.1 "Guards").
//
// Public routes pass without a request. Every other route needs `ensureQueryData(meQuery)`:
//  - a 401 redirects to `/login?redirect=<path>` (same-origin relative paths only, redirect.ts);
//  - a `recoveryOnly` session is confined to `/recover` (the add-passkey step);
//  - `recoveryAckAt === null` confines the session to `/settings/profile`, where the /settings layout route shows the
//    forced recovery-codes regenerate screen instead of the settings shell, before anything else. A `monoptah.lowCodes` flag (set by a
//    recover with 2 or fewer codes left, lowCodes.ts) does the same until new codes were created.
//
// Fragments (read in the route components, never sent to the server): `#invite=CODE` on /register,
// and `#fp=<hex>` on /pair/:code and /desktops/:id/verify. `#fp` is saved to sessionStorage before
// a redirect to /login and restored by `postSignInTarget` after sign-in.
import type { QueryClient } from "@tanstack/react-query";
import { redirect, type ParsedLocation } from "@tanstack/react-router";
import { restoreAccountSnapshots } from "@/features/offline/accountCache";
import { ApiError } from "./api";
import { isRateLimited, MAX_RATE_LIMIT_RETRIES, rateLimitWaitMs } from "@/lib/rateLimit";
import { releaseDevice } from "./deviceBinding";
import { hasLowCodes } from "./lowCodes";
import { meQuery, retryUnlessUnauthorizedBase } from "./queries";
import { loginRedirectTarget, saveFingerprintFragment } from "./redirect";

export interface AuthGuardArgs {
  location: ParsedLocation;
  context: { queryClient: QueryClient };
}

export type AuthGuard = (args: AuthGuardArgs) => void | Promise<void>;

export const PUBLIC_PATHS: readonly string[] = ["/login", "/register", "/recover", "/install"];
/** Where a session with `recoveryAckAt === null` must go before anything else (the /settings layout shows the regenerate gate). */
export const RECOVERY_REGENERATE_PATH = "/settings/profile";
export const RECOVERY_ADD_PASSKEY_PATH = "/recover";

function normalize(pathname: string): string {
  return pathname.length > 1 && pathname.endsWith("/") ? pathname.slice(0, -1) : pathname;
}

export function isPublicPath(pathname: string): boolean {
  return PUBLIC_PATHS.includes(normalize(pathname));
}

/** A 429 on `/api/me` never crashes the app (the limit is 120 calls per 60 s per session). */
async function loadMe(queryClient: QueryClient) {
  // The query itself does not retry a 429: this loop does, so a saved account can be used at once instead of after the waits.
  const options = { ...meQuery, retry: (count: number, error: Error) => !isRateLimited(error) && retryUnlessUnauthorizedBase(count, error) };
  for (let attempt = 0; ; attempt++) {
    try {
      return await queryClient.ensureQueryData(options);
    } catch (error) {
      if (!isRateLimited(error)) throw error;
      // The last account answer (same snapshot as the offline start) keeps the app usable while the limit lasts. This is
      // safe: the Worker resolves the session before the limit (middleware/auth.ts), so a 401 or a blocked account still
      // wins over a 429 and never reaches this branch.
      const saved = await restoreAccountSnapshots(queryClient).catch(() => undefined);
      if (saved) return saved;
      if (attempt >= MAX_RATE_LIMIT_RETRIES) throw error;
      await new Promise((resolve) => setTimeout(resolve, rateLimitWaitMs(error, attempt)));
    }
  }
}

export const authGuard: AuthGuard = async ({ location, context }) => {
  const pathname = normalize(location.pathname);
  if (isPublicPath(pathname)) return;

  let me;
  try {
    me = await loadMe(context.queryClient);
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
      // The session is over: the encrypted cache, the snapshots and the push subscription go with it.
      void releaseDevice();
      saveFingerprintFragment(pathname, location.hash);
      throw redirect({
        to: "/login",
        search: { redirect: loginRedirectTarget(pathname, location.searchStr) },
        replace: true,
      });
    }
    // The registry does not let a recovery-only session call GET /api/me: 403 `recovery_only`
    // means exactly that session, so it is confined like `me.recoveryOnly` below.
    if (error instanceof ApiError && error.status === 403 && error.code === "recovery_only") {
      throw redirect({ to: RECOVERY_ADD_PASSKEY_PATH, replace: true });
    }
    // No network at all (offline start, F-05): the last account answer, if one was saved, keeps the app
    // usable on cached data. Only `network` does this here (a 429 uses the snapshot in `loadMe`): a 401 or any other
    // answer of the server never does.
    if (error instanceof ApiError && error.status === 0 && error.code === "network") {
      me = await restoreAccountSnapshots(context.queryClient);
    }
    if (!me) throw error;
  }

  if (me.recoveryOnly) {
    throw redirect({ to: RECOVERY_ADD_PASSKEY_PATH, replace: true });
  }
  if ((me.recoveryAckAt === null || hasLowCodes(me.user.handle)) && pathname !== RECOVERY_REGENERATE_PATH) {
    throw redirect({ to: RECOVERY_REGENERATE_PATH, replace: true });
  }
};
