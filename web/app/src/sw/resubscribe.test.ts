import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { toBase64Url } from "./base64url";
import { loadPushState, savePushState } from "./pushState";
import { resubscribe } from "./resubscribe";

const VAPID = toBase64Url(new Uint8Array([4, ...Array.from({ length: 64 }, (_, i) => i)]));
const P256DH = toBase64Url(new Uint8Array([4, ...Array.from({ length: 64 }, () => 7)]));
const AUTH = toBase64Url(new Uint8Array(16).fill(9));

function subscription(endpoint = "https://fcm.googleapis.com/send/new") {
  return { toJSON: () => ({ endpoint, keys: { p256dh: P256DH, auth: AUTH } }) } as unknown as PushSubscription;
}

function fetcher(subscribeBody: { id: string } | number = { id: "sub-new" }) {
  return vi.fn((input: string, init?: RequestInit) => {
    void init;
    if (input === "/api/push/vapid-key") return Promise.resolve(Response.json({ publicKey: VAPID, keyId: "k2" }));
    if (input === "/api/push/subscriptions") {
      return Promise.resolve(typeof subscribeBody === "number" ? Response.json({ error: "x", code: "unauthorized" }, { status: subscribeBody }) : Response.json(subscribeBody));
    }
    return Promise.resolve(Response.json({ ok: true }));
  });
}

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
});

describe("resubscribe", () => {
  it("does nothing when this browser never enabled push", async () => {
    const subscribe = vi.fn();
    expect(await resubscribe({ pushManager: { subscribe }, fetch: fetcher() })).toBe(false);
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("subscribes with the current key, posts it with CSRF, and deletes the old row", async () => {
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-old", userId: "usr-1" });
    const subscribe = vi.fn(() => Promise.resolve(subscription()));
    const fetch = fetcher();
    expect(await resubscribe({ pushManager: { subscribe }, fetch })).toBe(true);

    const options = (subscribe.mock.calls as unknown as Array<[PushSubscriptionOptionsInit]>)[0]![0];
    expect(options.userVisibleOnly).toBe(true);
    expect(toBase64Url(options.applicationServerKey as Uint8Array)).toBe(VAPID);

    const post = fetch.mock.calls.find(([url]) => url === "/api/push/subscriptions")!;
    expect(post[1]).toMatchObject({ method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json", "X-Monoptah-CSRF": "1" } });
    expect(JSON.parse(post[1]!.body as string)).toEqual({ endpoint: "https://fcm.googleapis.com/send/new", keys: { p256dh: P256DH, auth: AUTH }, deviceId: "dev-1", vapidKeyId: "k2" });

    const del = fetch.mock.calls.find(([url]) => url === "/api/push/subscriptions/sub-old")!;
    expect(del[1]).toMatchObject({ method: "DELETE", headers: { "X-Monoptah-CSRF": "1" } });
    expect(await loadPushState()).toEqual({ deviceId: "dev-1", subscriptionId: "sub-new", userId: "usr-1", vapidKeyId: "k2" });
  });

  it("keeps the row when the endpoint is unchanged (same id)", async () => {
    await savePushState({ deviceId: "dev-1", subscriptionId: "same", userId: "usr-1" });
    const fetch = fetcher({ id: "same" });
    await resubscribe({ pushManager: { subscribe: () => Promise.resolve(subscription()) }, fetch });
    expect(fetch.mock.calls.some(([, init]) => init?.method === "DELETE")).toBe(false);
  });

  it("forgets the saved state when the session is gone", async () => {
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-old", userId: "usr-1" });
    await expect(resubscribe({ pushManager: { subscribe: () => Promise.resolve(subscription()) }, fetch: fetcher(401) })).rejects.toThrow("401");
    expect(await loadPushState()).toBeNull();
  });
});
