/// <reference lib="webworker" />
// Workbox precaching service worker, the shape of architecture.md 3.5 (precacheAndRoute).
import { precacheAndRoute } from "workbox-precaching";

declare const self: ServiceWorkerGlobalScope;

precacheAndRoute([{ url: "/index.html", revision: "spike-1" }]);
self.addEventListener("install", () => { void self.skipWaiting(); });
self.addEventListener("activate", (e) => { e.waitUntil(self.clients.claim()); });
