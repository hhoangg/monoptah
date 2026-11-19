import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockServer, MOCK_VAPID_KEY_ID, MOCK_VAPID_PUBLIC_KEY } from "@/features/accounts/mock/transport";
import { fromBase64Url, toBase64Url } from "@/sw/base64url";
import { loadPushState, savePushState } from "@/sw/pushState";
import { PushUnavailableError, forgetPushLocally, getSavedPushSubscription, hasBrowserSubscription, resubscribeIfVapidKeyChanged, subscribeToPush, SW_READY_TIMEOUT_MS, unsubscribeFromPush } from "./subscribe";

const keyBytes = fromBase64Url(MOCK_VAPID_PUBLIC_KEY);

function fakeSubscription(endpoint: string, key: Uint8Array = keyBytes) {
  return {
    endpoint,
    options: { applicationServerKey: key.buffer },
    getKey: (name: string) => (name === "p256dh" ? new Uint8Array([4, ...new Uint8Array(64).fill(1)]).buffer : new Uint8Array(16).fill(2).buffer),
    unsubscribe: vi.fn(() => Promise.resolve(true)),
  };
}

function install(existing: ReturnType<typeof fakeSubscription> | null, permission: NotificationPermission = "granted") {
  let current = existing;
  const subscribe = vi.fn(() => {
    current = fakeSubscription("https://fcm.googleapis.com/send/abc");
    return Promise.resolve(current);
  });
  const pushManager = { getSubscription: () => Promise.resolve(current), subscribe };
  const registration = { pushManager };
  Object.defineProperty(navigator, "serviceWorker", { configurable: true, value: { ready: Promise.resolve(registration), getRegistration: () => Promise.resolve(registration) } });
  vi.stubGlobal("PushManager", class {});
  vi.stubGlobal("Notification", { permission });
  return { subscribe };
}

let restore: (() => void) | undefined;
beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  restore = setApiTransport(createMockServer({ signedIn: true }).transport);
});
afterEach(() => {
  restore?.();
  Reflect.deleteProperty(navigator, "serviceWorker");
  vi.unstubAllGlobals();
});

describe("subscribeToPush", () => {
  it("never asks for permission and refuses when it is not granted", async () => {
    install(null, "default");
    await expect(subscribeToPush("dev-1", "usr-1")).rejects.toMatchObject({ reason: "permission" });
    await expect(subscribeToPush("dev-1", "usr-1")).rejects.toBeInstanceOf(PushUnavailableError);
  });

  it("gives up with `unsupported` when no service worker ever becomes ready", async () => {
    vi.useFakeTimers();
    try {
      install(null);
      Object.defineProperty(navigator, "serviceWorker", { configurable: true, value: { ready: new Promise(() => undefined) } });
      const result = subscribeToPush("dev-1", "usr-1");
      const settled = expect(result).rejects.toMatchObject({ reason: "unsupported" });
      await vi.advanceTimersByTimeAsync(SW_READY_TIMEOUT_MS + 1);
      await settled;
    } finally {
      vi.useRealTimers();
    }
  });

  it("refuses when push is unsupported", async () => {
    await expect(subscribeToPush("dev-1", "usr-1")).rejects.toMatchObject({ reason: "unsupported" });
  });

  it("subscribes with the Worker key, registers it and saves the ids", async () => {
    const { subscribe } = install(null);
    const id = await subscribeToPush("dev-1", "usr-1");
    const options = (subscribe.mock.calls as unknown as Array<[PushSubscriptionOptionsInit]>)[0]![0];
    expect(toBase64Url(options.applicationServerKey as Uint8Array)).toBe(MOCK_VAPID_PUBLIC_KEY);
    expect(await loadPushState()).toEqual({ deviceId: "dev-1", subscriptionId: id, userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });
    expect(await getSavedPushSubscription()).toEqual({ deviceId: "dev-1", subscriptionId: id, userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });
  });

  it("replaces a subscription made with another key", async () => {
    const stale = fakeSubscription("https://fcm.googleapis.com/send/old", new Uint8Array([4, ...new Uint8Array(64).fill(5)]));
    const { subscribe } = install(stale);
    await subscribeToPush("dev-1", "usr-1");
    expect(stale.unsubscribe).toHaveBeenCalledOnce();
    expect(subscribe).toHaveBeenCalledOnce();
  });

  it("reuses a subscription that already matches", async () => {
    const same = fakeSubscription("https://fcm.googleapis.com/send/same");
    const { subscribe } = install(same);
    await subscribeToPush("dev-1", "usr-1");
    expect(subscribe).not.toHaveBeenCalled();
    expect(same.unsubscribe).not.toHaveBeenCalled();
  });
});

describe("unsubscribeFromPush", () => {
  it("drops the browser subscription, the Worker row and the saved state", async () => {
    const existing = fakeSubscription("https://fcm.googleapis.com/send/same");
    install(existing);
    await subscribeToPush("dev-1", "usr-1");
    await unsubscribeFromPush();
    expect(existing.unsubscribe).toHaveBeenCalledOnce();
    expect(await loadPushState()).toBeNull();
  });

  it("clears the saved state even when the Worker call fails, so nothing subscribes again silently", async () => {
    const existing = fakeSubscription("https://fcm.googleapis.com/send/same");
    install(existing);
    await subscribeToPush("dev-1", "usr-1");
    restore?.();
    restore = setApiTransport(() => Promise.reject(new TypeError("offline")));
    await expect(unsubscribeFromPush()).rejects.toBeDefined();
    expect(existing.unsubscribe).toHaveBeenCalledOnce();
    expect(await loadPushState()).toBeNull();
  });

  it("is safe when nothing was subscribed", async () => {
    await expect(unsubscribeFromPush()).resolves.toBeUndefined();
  });
});

describe("forgetPushLocally", () => {
  it("drops the browser subscription and the saved state without calling the Worker", async () => {
    const existing = fakeSubscription("https://fcm.googleapis.com/send/same");
    install(existing);
    await subscribeToPush("dev-1", "usr-1");
    expect(await hasBrowserSubscription()).toBe(true);
    const calls: string[] = [];
    restore?.();
    restore = setApiTransport((url) => {
      calls.push(url);
      return Promise.reject(new Error("must not be called"));
    });
    await forgetPushLocally();
    expect(existing.unsubscribe).toHaveBeenCalledOnce();
    expect(await loadPushState()).toBeNull();
    expect(calls).toEqual([]);
  });
});

describe("mock push routes", () => {
  it("serves the key, keeps one row per endpoint and rejects a stale key id", async () => {
    const { transport } = createMockServer({ signedIn: true });
    const headers = { "Content-Type": "application/json", "X-Monoptah-CSRF": "1" };
    const keys = { p256dh: toBase64Url(new Uint8Array(65)), auth: toBase64Url(new Uint8Array(16)) };
    const body = (vapidKeyId: string) => JSON.stringify({ endpoint: "https://fcm.googleapis.com/send/x", keys, deviceId: "d1", vapidKeyId });
    expect(await (await transport("/api/push/vapid-key", { method: "GET" })).json()).toEqual({ publicKey: MOCK_VAPID_PUBLIC_KEY, keyId: MOCK_VAPID_KEY_ID });
    const first = (await (await transport("/api/push/subscriptions", { method: "POST", headers, body: body(MOCK_VAPID_KEY_ID) })).json()) as { id: string };
    const second = (await (await transport("/api/push/subscriptions", { method: "POST", headers, body: body(MOCK_VAPID_KEY_ID) })).json()) as { id: string };
    expect(second.id).toBe(first.id);
    expect((await transport("/api/push/subscriptions", { method: "POST", headers, body: body("old") })).status).toBe(400);
    expect((await transport(`/api/push/subscriptions/${first.id}`, { method: "DELETE", headers })).status).toBe(200);
    expect((await transport(`/api/push/subscriptions/${first.id}`, { method: "DELETE", headers })).status).toBe(404);
  });
});

describe("resubscribeIfVapidKeyChanged racing with the user turning push off", () => {
  it("does not keep a subscription or save state when push is turned off while the browser is subscribing", async () => {
    let release: (() => void) | undefined;
    const created = fakeSubscription("https://fcm.googleapis.com/send/late");
    let current: ReturnType<typeof fakeSubscription> | null = null;
    const subscribe = vi.fn(
      () =>
        new Promise<ReturnType<typeof fakeSubscription>>((resolve) => {
          release = () => {
            current = created;
            resolve(created);
          };
        }),
    );
    const registration = { pushManager: { getSubscription: () => Promise.resolve(current), subscribe } };
    Object.defineProperty(navigator, "serviceWorker", { configurable: true, value: { ready: Promise.resolve(registration), getRegistration: () => Promise.resolve(registration) } });
    vi.stubGlobal("PushManager", class {});
    vi.stubGlobal("Notification", { permission: "granted" });
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });

    const running = resubscribeIfVapidKeyChanged("usr-1");
    await vi.waitFor(() => expect(subscribe).toHaveBeenCalledOnce());
    // The user turns notifications off while the browser is still creating the subscription.
    await unsubscribeFromPush();
    expect(await loadPushState()).toBeNull();
    release?.();
    expect(await running).toBe(false);
    expect(created.unsubscribe, "the late browser subscription is taken back").toHaveBeenCalled();
    expect(await loadPushState(), "no state is saved against the user's choice").toBeNull();
  });

  it("does not subscribe at all when push was turned off while the key was being fetched", async () => {
    const { subscribe } = install(null);
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-1", vapidKeyId: "retired-key" });
    const running = resubscribeIfVapidKeyChanged("usr-1");
    await unsubscribeFromPush();
    expect(await running).toBe(false);
    expect(subscribe).not.toHaveBeenCalled();
    expect(await loadPushState()).toBeNull();
  });
});

describe("resubscribeIfVapidKeyChanged", () => {
  it("does nothing when this browser never enabled push", async () => {
    const { subscribe } = install(null);
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(false);
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("keeps the subscription while the server key id is the same", async () => {
    const same = fakeSubscription("https://fcm.googleapis.com/send/same");
    const { subscribe } = install(same);
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(false);
    expect(subscribe).not.toHaveBeenCalled();
    expect(same.unsubscribe).not.toHaveBeenCalled();
  });

  it("subscribes again and records the new key id when the server key id changed", async () => {
    const stale = fakeSubscription("https://fcm.googleapis.com/send/old", new Uint8Array([4, ...new Uint8Array(64).fill(5)]));
    const { subscribe } = install(stale);
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-old", userId: "usr-1", vapidKeyId: "retired-key" });
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(true);
    expect(stale.unsubscribe).toHaveBeenCalledOnce();
    expect(subscribe).toHaveBeenCalledOnce();
    expect(await loadPushState()).toMatchObject({ deviceId: "dev-1", userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });
  });

  it("subscribes again, without a gesture, when the key is the same but the browser lost its subscription", async () => {
    const { subscribe } = install(null);
    const announced = vi.fn();
    window.addEventListener("monoptah:push-resubscribed", announced);
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(true);
    window.removeEventListener("monoptah:push-resubscribed", announced);
    expect(announced).toHaveBeenCalledOnce();
    expect(subscribe).toHaveBeenCalledOnce();
    expect(await loadPushState()).toMatchObject({ deviceId: "dev-1", userId: "usr-1", vapidKeyId: MOCK_VAPID_KEY_ID });
  });

  it("does not subscribe again when the permission is not granted", async () => {
    const { subscribe } = install(null, "default");
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-1", vapidKeyId: "retired-key" });
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(false);
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("treats a state saved without a key id as changed once", async () => {
    install(fakeSubscription("https://fcm.googleapis.com/send/same"));
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-1" });
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(true);
    expect((await loadPushState())?.vapidKeyId).toBe(MOCK_VAPID_KEY_ID);
  });

  it("leaves another account's state alone and never throws", async () => {
    const { subscribe } = install(null);
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "usr-2", vapidKeyId: "retired-key" });
    expect(await resubscribeIfVapidKeyChanged("usr-1")).toBe(false);
    expect(subscribe).not.toHaveBeenCalled();
  });
});
