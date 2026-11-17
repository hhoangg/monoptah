// Origin and CSRF checks (security.md 3.4) for cookie-authenticated routes, and cookie stripping for
// the routes that must never read a cookie (architecture.md 4.1).
import type { MiddlewareHandler } from "hono";
import type { ApiRoute } from "@monoptah/protocol";
import { apiError, type AppEnv } from "../http.ts";

/** The custom header every non-GET browser call carries; it forces a CORS preflight the Worker never grants. */
export const CSRF_HEADER = "X-Monoptah-CSRF";

/**
 * A route authenticated by the session cookie: session, fresh, admin and session+grant routes, and the
 * public browser auth routes under `/api/auth/`. Token routes (`/api/desktop/*`, `/api/push/notify`)
 * and the desktop pairing routes (`/api/pair/start`, `/api/pair/poll`) are not: the desktop app sends
 * no `Origin` and no cookie.
 */
export function isCookieAuthenticated(route: Pick<ApiRoute, "auth" | "path">): boolean {
  if (route.auth === "session" || route.auth === "fresh" || route.auth === "admin" || route.auth === "session+grant") return true;
  return route.auth === "public" && route.path.startsWith("/api/auth/");
}

/** Safe methods carry no state change, so the checks apply to every other method. */
function isNonGet(method: string): boolean {
  return method !== "GET" && method !== "HEAD" && method !== "OPTIONS";
}

/** Rejects a non-GET request whose `Origin` is not `RP_ORIGIN` (missing counts as wrong) or without `X-Monoptah-CSRF: 1`. */
export const originAndCsrf: MiddlewareHandler<AppEnv> = async (c, next) => {
  if (isNonGet(c.req.method)) {
    if (c.req.header("Origin") !== c.env.RP_ORIGIN) {
      return apiError(c, 403, "bad_origin", "Origin not allowed");
    }
    if (c.req.header(CSRF_HEADER) !== "1") {
      return apiError(c, 403, "csrf", "Missing CSRF header");
    }
  }
  return next();
};

/**
 * For token and pairing routes: removes the `Cookie` header from the request before anything can read it,
 * so a browser request can never ride a session cookie into them.
 */
export const ignoreCookies: MiddlewareHandler<AppEnv> = async (c, next) => {
  if (c.req.raw.headers.has("Cookie")) {
    const headers = new Headers(c.req.raw.headers);
    headers.delete("Cookie");
    c.req.raw = new Request(c.req.raw, { headers });
  }
  await next();
};

/** The browser checks that apply to `route` (none for token and pairing routes). */
export function browserChecks(route: Pick<ApiRoute, "auth" | "path">): MiddlewareHandler<AppEnv> {
  return isCookieAuthenticated(route) ? originAndCsrf : ignoreCookies;
}
