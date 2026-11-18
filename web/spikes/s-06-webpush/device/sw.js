/* global self */
// S-06 service worker: shows every push. The one exception is a test-only message `{"silent":true}` that
// deliberately shows nothing, to observe how iOS and Safari react to an invisible push (they revoke the permission).
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let data;
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    data = { body: event.data ? event.data.text() : "" };
  }
  if (data.silent === true) return;
  event.waitUntil(
    self.registration.showNotification(data.title || "Monoptah push spike", {
      body: data.body || "Push received",
      tag: data.tag,
    }),
  );
});
