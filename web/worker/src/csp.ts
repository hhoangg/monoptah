// The CSP and the companion security headers of security.md 8.1. Dependency free on purpose: the
// app build (web/app) imports this file to generate `_headers`, so the policy exists in one place.

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
