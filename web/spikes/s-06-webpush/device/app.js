/* global window, document, navigator, location, Notification, atob, URL, URLSearchParams */
// S-06 device test page. No inline script, no innerHTML: it works under the CSP and Trusted Types rules of
// web/worker/src/csp.ts (script-src 'self', require-trusted-types-for 'script', trusted-types default).
const $ = (id) => document.getElementById(id);
const status = $("status");
const out = $("subscription");

function log(message) {
  status.textContent = `${status.textContent}${status.textContent ? "\n" : ""}${new Date().toISOString().slice(11, 19)} ${message}`;
}

// Trusted Types: ServiceWorkerContainer.register takes a script URL, which a `trusted-types default` page must supply
// through a policy named "default". Only same-origin URLs are allowed.
if (window.trustedTypes && window.trustedTypes.createPolicy && !window.trustedTypes.defaultPolicy) {
  window.trustedTypes.createPolicy("default", {
    createScriptURL(url) {
      if (new URL(url, location.href).origin !== location.origin) throw new TypeError("cross-origin script URL");
      return url;
    },
  });
}

function base64UrlToBytes(text) {
  const padded = (text + "=".repeat((4 - (text.length % 4)) % 4)).replaceAll("-", "+").replaceAll("_", "/");
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}

const supported = "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
$("support").textContent = supported
  ? `serviceWorker, PushManager, Notification present. standalone=${String(window.matchMedia("(display-mode: standalone)").matches || navigator.standalone === true)} secure=${String(window.isSecureContext)} permission=${Notification.permission}`
  : "Push is NOT available here (on iOS it needs iOS 16.4+ and an app added to the Home Screen; it needs an https origin).";

async function subscribe() {
  const key = $("vapid").value.trim();
  if (!key) {
    log("Enter the VAPID public key first.");
    return;
  }
  // Both calls run inside the click handler: Safari and iOS require the permission request in a user gesture.
  const permission = await Notification.requestPermission();
  log(`Notification.requestPermission() -> ${permission}`);
  if (permission !== "granted") return;
  const registration = await navigator.serviceWorker.register("sw.js");
  await navigator.serviceWorker.ready;
  const existing = await registration.pushManager.getSubscription();
  if (existing) await existing.unsubscribe();
  const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: base64UrlToBytes(key) });
  out.textContent = JSON.stringify(subscription.toJSON(), null, 2);
  $("copy").disabled = false;
  log("Subscribed. Copy the JSON into a file and run send.ts with it.");
}

$("enable").addEventListener("click", () => {
  subscribe().catch((error) => log(`Error: ${error && error.name}: ${error && error.message}`));
});

$("copy").addEventListener("click", () => {
  navigator.clipboard.writeText(out.textContent || "").then(
    () => log("Copied."),
    (error) => log(`Copy failed: ${error && error.message}`),
  );
});

if (new URLSearchParams(location.search).get("auto") === "1" && supported) {
  // Deliberately NOT in a gesture, to record what the platform does (see FINDINGS.md).
  Notification.requestPermission().then(
    (permission) => log(`auto: requestPermission() outside a gesture -> ${permission}`),
    (error) => log(`auto: requestPermission() threw ${error && error.name}: ${error && error.message}`),
  );
}
