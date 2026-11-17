// Subscribe and unsubscribe for Task 10 (P-05). Permission is NOT requested here: the caller asks for it
// from a click (a user gesture, required on iOS) and calls `subscribeToPush` only after it is granted.
import { apiFetch } from "@/features/accounts/api";
import { fromBase64Url, toBase64Url } from "@/sw/base64url";
import { clearPushState, loadPushState, savePushState } from "@/sw/pushState";
import { pushEndpoints } from "./endpoints";

export type PushUnavailableReason = "unsupported" | "permission";

export class PushUnavailableError extends Error {
  readonly reason: PushUnavailableReason;
  constructor(reason: PushUnavailableReason) {
    super(reason === "permission" ? "Notification permission is not granted" : "Push is not supported in this browser");
    this.name = "PushUnavailableError";
    this.reason = reason;
  }
}

/** True when this browser can subscribe (service worker, Push API and Notifications). iOS needs the installed PWA. */
export function isPushSupported(): boolean {
  return typeof navigator !== "undefined" && "serviceWorker" in navigator && typeof PushManager !== "undefined" && typeof Notification !== "undefined";
}

function sameKey(a: ArrayBuffer | null, b: Uint8Array): boolean {
  if (!a) return false;
  const left = new Uint8Array(a);
  return left.length === b.length && left.every((byte, index) => byte === b[index]);
}

/**
 * Subscribes this browser and registers the subscription with the Worker for `deviceId` (the server device id
 * of the browser, `ensureDevice().deviceId`). Returns the server subscription id. A subscription made with
 * an older VAPID key is replaced (the key id check of `POST /api/push/subscriptions`).
 */
export async function subscribeToPush(deviceId: string): Promise<string> {
  if (!isPushSupported()) throw new PushUnavailableError("unsupported");
  if (Notification.permission !== "granted") throw new PushUnavailableError("permission");

  const registration = await navigator.serviceWorker.ready;
  const vapid = await apiFetch(pushEndpoints.vapidKey);
  const applicationServerKey = fromBase64Url(vapid.publicKey);

  let subscription = await registration.pushManager.getSubscription();
  if (subscription && !sameKey(subscription.options.applicationServerKey, applicationServerKey)) {
    await subscription.unsubscribe();
    subscription = null;
  }
  subscription ??= await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey });

  const p256dh = subscription.getKey("p256dh");
  const auth = subscription.getKey("auth");
  if (!p256dh || !auth) throw new PushUnavailableError("unsupported");
  const { id } = await apiFetch(pushEndpoints.subscribe, {
    body: { endpoint: subscription.endpoint, keys: { p256dh: toBase64Url(p256dh), auth: toBase64Url(auth) }, deviceId, vapidKeyId: vapid.keyId },
  });
  await savePushState({ deviceId, subscriptionId: id });
  return id;
}

/** Removes this browser's subscription: the browser side first, then the Worker row. Safe when not subscribed. */
export async function unsubscribeFromPush(): Promise<void> {
  const state = await loadPushState();
  if (isPushSupported()) {
    const registration = await navigator.serviceWorker.getRegistration();
    const subscription = await registration?.pushManager.getSubscription();
    await subscription?.unsubscribe();
  }
  if (state) {
    // A 404 means the row is already gone (revoked device, cleaned up): that is the goal.
    await apiFetch(pushEndpoints.unsubscribe, { params: { id: state.subscriptionId } }).catch((error: unknown) => {
      if (!(error instanceof Error && "status" in error && error.status === 404)) throw error;
    });
  }
  await clearPushState();
}

/** The saved subscription of this browser (server ids), or null; for the settings toggle of Task 10. */
export async function getSavedPushSubscription() {
  return loadPushState();
}
