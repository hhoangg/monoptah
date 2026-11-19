// Security headers (security.md 8.1) and the CSP builder.
import type { MiddlewareHandler } from "hono";
import type { AppEnv } from "../http.ts";
import { buildCsp, COMPANION_HEADERS } from "../csp.ts";

export { buildCsp, COMPANION_HEADERS };

function isNoStorePath(pathname: string): boolean {
  return pathname.startsWith("/api/") || pathname === "/api" || pathname.startsWith("/relay/") || pathname === "/relay";
}

/**
 * First middleware. Sets the companion headers, `Cache-Control: no-store` on `/api/*` and `/relay/*`,
 * and the CSP on any HTML response the Worker itself returns (HTML served from the assets binding
 * gets its headers from `app/dist/client/_headers`; the Worker only sees HTML when it falls through to ASSETS).
 * A WebSocket upgrade response (101) is left untouched: its headers are immutable.
 */
export const securityHeaders: MiddlewareHandler<AppEnv> = async (c, next) => {
  await next();
  const res = c.res;
  if (res.status === 101) return;
  // `c.res` may come from `ASSETS.fetch` (immutable headers): copy the headers into a new response.
  const headers = new Headers(res.headers);
  for (const [name, value] of Object.entries(COMPANION_HEADERS)) headers.set(name, value);
  const url = new URL(c.req.url);
  if (isNoStorePath(url.pathname)) headers.set("Cache-Control", "no-store");
  if ((headers.get("Content-Type") ?? "").toLowerCase().startsWith("text/html")) {
    headers.set("Content-Security-Policy", buildCsp(url.host));
  }
  // Hono's `c.res` setter merges the headers of the previous response over the new one (hono dist/context.js),
  // so an ASSETS response would keep its own CSP (W13-1). Clearing it first makes the headers set here win.
  c.res = undefined as unknown as Response;
  c.res = new Response(res.body, { status: res.status, statusText: res.statusText, headers });
};
