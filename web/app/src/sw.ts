// Service worker (W-12, architecture.md 3.5). Built by vite-plugin-pwa with `injectManifest`: the
// precache list replaces `self.__WB_MANIFEST` at build time, and the result is emitted as /sw.js.
//
// Structure (Task 9 adds the push handlers in section 3):
//  1. precache and navigation fallback (offline start)
//  2. update protocol: no skipWaiting on install; the page asks with a SKIP_WAITING message
//  3. event handlers: add `push`, `notificationclick` and `pushsubscriptionchange` here or in a module
//     under src/sw/ imported at the top and registered with `self.addEventListener` in this section
//
// Never cached: /api/* and /relay/* (src/sw/routing.ts). There are no runtime caching routes.
import { clientsClaim } from "workbox-core";
import { cleanupOutdatedCaches, createHandlerBoundToURL, precacheAndRoute } from "workbox-precaching";
import { NavigationRoute, registerRoute } from "workbox-routing";
import { NAVIGATION_DENYLIST, NAVIGATION_FALLBACK } from "./sw/routing";

declare const self: ServiceWorkerGlobalScope & { __WB_MANIFEST: Array<string | { url: string; revision: string | null }> };

// --- 1. Precache and navigation fallback ---

cleanupOutdatedCaches();
precacheAndRoute(self.__WB_MANIFEST);
registerRoute(new NavigationRoute(createHandlerBoundToURL(NAVIGATION_FALLBACK), { denylist: [...NAVIGATION_DENYLIST] }));

// --- 2. Update protocol ---

// A new worker waits until the user taps Reload (features/offline/swUpdate.ts): a link that is running or
// a message that is being sent must not be cut by an update the user did not ask for.
self.addEventListener("message", (event) => {
  const data: unknown = event.data;
  if (typeof data === "object" && data !== null && (data as { type?: unknown }).type === "SKIP_WAITING") {
    void self.skipWaiting();
  }
});

// The first install has nothing to wait for; claiming lets that very page load work offline on the next
// navigation without a second reload. After an update the page reloads on `controlling` anyway.
clientsClaim();

// --- 3. Push handlers (Task 9, P-04) go here ---
