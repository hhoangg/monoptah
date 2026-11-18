// Re-subscribe after `pushsubscriptionchange` (architecture.md 3.5): the browser dropped or rotated the
// subscription, so the worker subscribes again with the current VAPID key and tells the Worker. It speaks
// the three routes of the push contract with plain fetch (no app code); the device id comes from pushState.
import { fromBase64Url } from "./base64url";
import { clearPushState, loadPushState, savePushState } from "./pushState";

export interface ResubscribeDeps {
  pushManager: Pick<PushManager, "subscribe">;
  fetch(input: string, init?: RequestInit): Promise<Response>;
}

const JSON_HEADERS = { "Content-Type": "application/json", "X-Monoptah-CSRF": "1" } as const;

async function readJson(response: Response): Promise<Record<string, unknown>> {
  const body: unknown = await response.json();
  if (typeof body !== "object" || body === null) throw new Error("Unexpected push response");
  return body as Record<string, unknown>;
}

/** Resolves to false when there is nothing to do (this browser never enabled push), true after a re-subscribe. */
export async function resubscribe(deps: ResubscribeDeps): Promise<boolean> {
  const state = await loadPushState();
  if (!state) return false;

  const keyResponse = await deps.fetch("/api/push/vapid-key", { credentials: "same-origin", headers: { Accept: "application/json" } });
  if (!keyResponse.ok) throw new Error(`vapid-key failed (${keyResponse.status})`);
  const key = await readJson(keyResponse);
  if (typeof key.publicKey !== "string" || typeof key.keyId !== "string") throw new Error("Unexpected vapid-key response");

  const subscription = await deps.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: fromBase64Url(key.publicKey) });
  const json = subscription.toJSON();
  const p256dh = json.keys?.p256dh;
  const auth = json.keys?.auth;
  if (!json.endpoint || !p256dh || !auth) throw new Error("Subscription has no keys");

  const post = await deps.fetch("/api/push/subscriptions", {
    method: "POST",
    credentials: "same-origin",
    headers: JSON_HEADERS,
    body: JSON.stringify({ endpoint: json.endpoint, keys: { p256dh, auth }, deviceId: state.deviceId, vapidKeyId: key.keyId }),
  });
  if (!post.ok) {
    // The account is signed out or the device was forgotten: the saved state is stale, drop it.
    if (post.status === 401 || post.status === 404) await clearPushState();
    throw new Error(`subscribe failed (${post.status})`);
  }
  const created = await readJson(post);
  if (typeof created.id !== "string") throw new Error("Unexpected subscribe response");

  // The same endpoint keeps its row (the Worker upserts by endpoint); delete the old one only when it differs.
  if (created.id !== state.subscriptionId) {
    await deps.fetch(`/api/push/subscriptions/${encodeURIComponent(state.subscriptionId)}`, { method: "DELETE", credentials: "same-origin", headers: JSON_HEADERS }).catch(() => undefined);
  }
  await savePushState({ deviceId: state.deviceId, subscriptionId: created.id, userId: state.userId });
  return true;
}
