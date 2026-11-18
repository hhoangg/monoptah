// What the service worker may cache and answer (architecture.md 3.5, security.md). Pure data and
// predicates, no service worker globals, so a unit test can read it and check-dist can compare it with
// the built sw.js. sw.ts is the only consumer that wires it into Workbox.

/** The shell document every navigation falls back to when offline. */
export const NAVIGATION_FALLBACK = "/index.html";

/**
 * Navigations that must reach the network and never get the shell: the API and the relay. A response of
 * either is private (cookies, ciphertext frames) and is never stored in Cache Storage.
 */
export const NAVIGATION_DENYLIST: readonly RegExp[] = [/^\/api\//, /^\/relay\//];

/**
 * Runtime caching routes. None, on purpose: the only things in Cache Storage are the precached build
 * assets. Adding a route here needs a security review; a test fails if one matches /api or /relay.
 */
export const RUNTIME_CACHE_ROUTES: readonly { readonly pattern: RegExp; readonly handler: string }[] = [];

/** True when a navigation to `pathname` may be answered with the cached shell. */
export function isShellNavigation(pathname: string): boolean {
  return !NAVIGATION_DENYLIST.some((pattern) => pattern.test(pathname));
}

/** True for paths that must never be served from or written to a cache by the service worker. */
export function isNeverCached(pathname: string): boolean {
  return /^\/(api|relay)(\/|$)/.test(pathname);
}
