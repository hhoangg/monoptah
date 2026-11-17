// Root `beforeLoad` auth guard (architecture.md 3.1 "Guards").
//
// Public routes pass without a request. Every other route needs `ensureQueryData(meQuery)`:
//  - a 401 redirects to `/login?redirect=<path>` (same-origin relative paths only, redirect.ts);
//  - a `recoveryOnly` session is confined to `/recover` (the add-passkey step);
//  - `recoveryAckAt === null` confines the session to `/settings`, which must show the forced
//    recovery-codes regenerate screen before anything else.
//
// Fragments (read in the route components, never sent to the server): `#invite=CODE` on /register,
// and `#fp=<hex>` on /pair/:code and /desktops/:id/verify. `#fp` is saved to sessionStorage before
// a redirect to /login and restored by `postSignInTarget` after sign-in.
import type { QueryClient } from "@tanstack/react-query";
import { redirect, type ParsedLocation } from "@tanstack/react-router";
import { ApiError } from "./api";
import { meQuery } from "./queries";
import { loginRedirectTarget, saveFingerprintFragment } from "./redirect";

export interface AuthGuardArgs {
  location: ParsedLocation;
  context: { queryClient: QueryClient };
}

export type AuthGuard = (args: AuthGuardArgs) => void | Promise<void>;

export const PUBLIC_PATHS: readonly string[] = ["/login", "/register", "/recover", "/install"];
/** Where a session with `recoveryAckAt === null` must go before anything else. */
export const RECOVERY_REGENERATE_PATH = "/settings";
export const RECOVERY_ADD_PASSKEY_PATH = "/recover";

function normalize(pathname: string): string {
  return pathname.length > 1 && pathname.endsWith("/") ? pathname.slice(0, -1) : pathname;
}

export function isPublicPath(pathname: string): boolean {
  return PUBLIC_PATHS.includes(normalize(pathname));
}

export const authGuard: AuthGuard = async ({ location, context }) => {
  const pathname = normalize(location.pathname);
  if (isPublicPath(pathname)) return;

  let me;
  try {
    me = await context.queryClient.ensureQueryData(meQuery);
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) {
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
    throw error;
  }

  if (me.recoveryOnly) {
    throw redirect({ to: RECOVERY_ADD_PASSKEY_PATH, replace: true });
  }
  if (me.recoveryAckAt === null && pathname !== RECOVERY_REGENERATE_PATH) {
    throw redirect({ to: RECOVERY_REGENERATE_PATH, replace: true });
  }
};
