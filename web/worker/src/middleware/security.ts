// Security headers (security.md 8.1) and the CSP builder.
import type { MiddlewareHandler } from "hono";
import type { AppEnv } from "../http.ts";

/**
 * The Content-Security-Policy of security.md 8.1, verbatim, with `HOST` substituted: some browsers do
 * not match `'self'` to `wss:` on the same host. `host` must be a bare `host[:port]` (a URL host).
 */
export function buildCsp(host: string): string {
  return [
    "default-src 'none'",
    "script-src 'self'",
    "style-src 'self'",
    "style-src-attr 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self'",
    `connect-src 'self' wss://${host}`,
    "worker-src 'self'",
    "manifest-src 'self'",
    "frame-ancestors 'none'",
    "base-uri 'none'",
    "form-action 'self'",
    "object-src 'none'",
    "require-trusted-types-for 'script'",
    "trusted-types default",
    "upgrade-insecure-requests",
  ].join("; ");
}

/** Companion headers of security.md 8.1, on every response of the Worker. */
export const COMPANION_HEADERS: Readonly<Record<string, string>> = {
  "Strict-Transport-Security": "max-age=63072000; includeSubDomains; preload",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Permissions-Policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Resource-Policy": "same-origin",
};

function isNoStorePath(pathname: string): boolean {
  return pathname.startsWith("/api/") || pathname === "/api" || pathname.startsWith("/relay/") || pathname === "/relay";
}

/**
 * First middleware. Sets the companion headers, `Cache-Control: no-store` on `/api/*` and `/relay/*`,
 * and the CSP on any HTML response the Worker itself returns (HTML served from the assets binding
 * gets its headers from `app/dist/_headers`; the Worker only sees HTML when it falls through to ASSETS).
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
  c.res = new Response(res.body, { status: res.status, statusText: res.statusText, headers });
};
