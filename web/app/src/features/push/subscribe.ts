// Subscribe and unsubscribe for Task 10 (P-05). Permission is NOT requested here: the caller asks for it
// from a click (a user gesture, required on iOS) and calls `subscribeToPush` only after it is granted.
import { apiFetch } from "@/features/accounts/api";
import { fromBase64Url, toBase64Url } from "@/sw/base64url";
import { clearPushState, loadPushState, savePushState } from "@/sw/pushState";
import { pushEndpoints } from "./endpoints";

/** Fired on `window` after this browser turned push on or off, so the settings re-read their state. */
export const PUSH_CHANGED_EVENT = "monoptah:push-changed";
/** Fired on `window` after a background run made the subscription again, so an open Settings card drops its "turn on again" note. */
export const PUSH_RESUBSCRIBED_EVENT = "monoptah:push-resubscribed";

/**
 * Bumped whenever the user turns push off (or it is forgotten). A background `resubscribeIfVapidKeyChanged` that started
 * earlier carries the value it saw and stops before it subscribes or saves anything against the user's choice.
 */
let pushEpoch = 0;

class PushAbortedError extends Error {
  constructor() {
    super("Push was turned off while it was being set up");
    this.name = "PushAbortedError";
  }
}

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

/** How long to wait for a service worker before saying push is unavailable (registration failed or is blocked). */
export const SW_READY_TIMEOUT_MS = 8000;

async function serviceWorkerReady(timeoutMs: number = SW_READY_TIMEOUT_MS): Promise<ServiceWorkerRegistration> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new PushUnavailableError("unsupported")), timeoutMs);
  });
  try {
    return await Promise.race([navigator.serviceWorker.ready, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

function sameKey(a: ArrayBuffer | null, b: Uint8Array): boolean {
  if (!a) return false;
  const left = new Uint8Array(a);
  return left.length === b.length && left.every((byte, index) => byte === b[index]);
}

/**
 * Subscribes this browser and registers the subscription with the Worker for `deviceId` (the server device id
 * of the browser, `ensureDevice().deviceId`) and remembers `userId` as its owner, so a later session of another
 * account (or none) can remove it (`forgetPushLocally`). Returns the server subscription id. A subscription made with
 * an older VAPID key is replaced (the key id check of `POST /api/push/subscriptions`).
 */
export async function subscribeToPush(deviceId: string, userId: string, options: { epoch?: number } = {}): Promise<string> {
  if (!isPushSupported()) throw new PushUnavailableError("unsupported");
  if (Notification.permission !== "granted") throw new PushUnavailableError("permission");
  // Only a background run carries an epoch: a click is the user's own choice and always wins.
  const aborted = (): boolean => options.epoch !== undefined && options.epoch !== pushEpoch;

  const registration = await serviceWorkerReady();
  const vapid = await apiFetch(pushEndpoints.vapidKey);
  if (aborted()) throw new PushAbortedError();
  const applicationServerKey = fromBase64Url(vapid.publicKey);

  let subscription = await registration.pushManager.getSubscription();
  if (subscription && !sameKey(subscription.options.applicationServerKey, applicationServerKey)) {
    await subscription.unsubscribe();
    subscription = null;
  }
  subscription ??= await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey });
  // The user turned push off while the browser was subscribing: take the new subscription back.
  const active = subscription;
  const abandon = async (serverId?: string): Promise<never> => {
    await active.unsubscribe().catch(() => undefined);
    if (serverId) await apiFetch(pushEndpoints.unsubscribe, { params: { id: serverId } }).catch(() => undefined);
    await clearPushState().catch(() => undefined);
    throw new PushAbortedError();
  };
  if (aborted()) return abandon();

  const p256dh = subscription.getKey("p256dh");
  const auth = subscription.getKey("auth");
  if (!p256dh || !auth) throw new PushUnavailableError("unsupported");
  const { id } = await apiFetch(pushEndpoints.subscribe, {
    body: { endpoint: subscription.endpoint, keys: { p256dh: toBase64Url(p256dh), auth: toBase64Url(auth) }, deviceId, vapidKeyId: vapid.keyId },
  });
  if (aborted()) return abandon(id);
  await savePushState({ deviceId, subscriptionId: id, userId, vapidKeyId: vapid.keyId });
  if (aborted()) return abandon(id);
  return id;
}

/**
 * Makes the subscription again when the server's VAPID `keyId` is not the one it was made with (the key was
 * rotated, architecture.md 8): a push signed with the new key would not reach the old subscription. Also makes it
 * again when the permission is granted but the browser holds no subscription. Does nothing
 * when this browser never enabled push, the permission is gone or the key is the same and the subscription exists. A state saved before the
 * key id was kept counts as changed once. Resolves to true after a new subscription; never throws.
 */
export async function resubscribeIfVapidKeyChanged(userId: string): Promise<boolean> {
  const epoch = pushEpoch;
  try {
    if (!isPushSupported() || Notification.permission !== "granted") return false;
    const state = await loadPushState();
    if (!state || (state.userId !== null && state.userId !== userId)) return false;
    const vapid = await apiFetch(pushEndpoints.vapidKey);
    // The browser can lose the subscription on its own (iOS drops it, the user cleared site data) while the
    // permission stays granted: subscribe again, no gesture is needed.
    if (state.vapidKeyId === vapid.keyId && (await hasBrowserSubscription())) return false;
    // `subscribeToPush` replaces a browser subscription made with another key and upserts the Worker row.
    // No user gesture here: iOS may refuse. The refusal is caught below and the caller's 60 s throttle keeps it from looping.
    if (epoch !== pushEpoch) return false;
    const id = await subscribeToPush(state.deviceId, userId, { epoch });
    if (id !== state.subscriptionId) {
      await apiFetch(pushEndpoints.unsubscribe, { params: { id: state.subscriptionId } }).catch(() => undefined);
    }
    if (typeof window !== "undefined") window.dispatchEvent(new Event(PUSH_RESUBSCRIBED_EVENT));
    return true;
  } catch {
    return false;
  }
}

/** Removes the browser-side subscription of the service worker registration. Never throws. */
async function unsubscribeInBrowser(): Promise<void> {
  try {
    if (!isPushSupported()) return;
    const registration = await navigator.serviceWorker.getRegistration();
    const subscription = await registration?.pushManager.getSubscription();
    await subscription?.unsubscribe();
  } catch {
    // The browser keeps it at worst; the server row dies with the first push the push service refuses.
  }
}

/**
 * Removes this browser's subscription: the browser side first, then the Worker row. Safe when not subscribed.
 * The saved state is cleared even when the Worker call fails: the browser subscription is gone by then, and a
 * stale state would make the next page load subscribe again without being asked. The error still propagates.
 */
export async function unsubscribeFromPush(): Promise<void> {
  pushEpoch += 1;
  const state = await loadPushState();
  await unsubscribeInBrowser();
  try {
    if (state) {
      // A 404 means the row is already gone (revoked device, cleaned up): that is the goal.
      await apiFetch(pushEndpoints.unsubscribe, { params: { id: state.subscriptionId } }).catch((error: unknown) => {
        if (!(error instanceof Error && "status" in error && error.status === 404)) throw error;
      });
    }
  } finally {
    await clearPushState().catch(() => undefined);
  }
}

/**
 * For a session that ended without "Sign out" (401, another account signing in): there is no session left to
 * call the Worker with, so only the browser side goes and the saved state is cleared. The server row is dropped
 * on the next send, when the push service answers 404 or 410 (architecture.md 8). Never throws.
 */
export async function forgetPushLocally(): Promise<void> {
  pushEpoch += 1;
  await unsubscribeInBrowser();
  await clearPushState().catch(() => undefined);
}

/** True when the browser itself holds a push subscription (the saved state alone could be stale). */
export async function hasBrowserSubscription(): Promise<boolean> {
  try {
    if (!isPushSupported()) return false;
    const registration = await navigator.serviceWorker.getRegistration();
    return (await registration?.pushManager.getSubscription()) != null;
  } catch {
    return false;
  }
}

/** The saved subscription of this browser (server ids), or null; for the settings toggle of Task 10. */
export async function getSavedPushSubscription() {
  return loadPushState();
}
