import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { QueryClient } from "@tanstack/react-query";
import type { ParsedLocation } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { startAccountPersistence } from "@/features/offline/accountCache";
import { EncryptedPersistAdapter } from "@/features/offline/encryptedPersist";
import { PAUSE_RECORD_KEY } from "@/features/push/pauseRecord";
import { loadPushState, savePushState } from "@/sw/pushState";
import { setApiTransport } from "./api";
import { authGuard } from "./authGuard";
import { bindDeviceToUser, OWNER_KEY, releaseDevice, resetDeviceBindingForTests } from "./deviceBinding";
import { createMockServer } from "./mock/transport";
import { installSessionExpiryHandler } from "./sessionExpiry";
import { meQuery, signOut } from "./queries";

let previous: PersistAdapter;
let restoreApi: (() => void) | undefined;
const unsubscribe = vi.fn(() => Promise.resolve(true));

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  previous = registerPersistAdapter(new EncryptedPersistAdapter());
  resetDeviceBindingForTests();
  localStorage.clear();
  unsubscribe.mockClear();
  const registration = { pushManager: { getSubscription: () => Promise.resolve({ unsubscribe }) } };
  Object.defineProperty(navigator, "serviceWorker", { configurable: true, value: { getRegistration: () => Promise.resolve(registration) } });
  vi.stubGlobal("PushManager", class {});
  vi.stubGlobal("Notification", { permission: "granted" });
});
afterEach(() => {
  registerPersistAdapter(previous);
  restoreApi?.();
  restoreApi = undefined;
  Reflect.deleteProperty(navigator, "serviceWorker");
  vi.unstubAllGlobals();
});

async function seed(owner: string | null) {
  await getPersistAdapter().set("catalog:desk-1", { projects: ["secret prompt"] });
  await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: owner });
  localStorage.setItem(PAUSE_RECORD_KEY, JSON.stringify({ "desk-1": { agentInput: true, agentFinished: true, monoMessages: true } }));
  if (owner) localStorage.setItem(OWNER_KEY, owner);
}

const cached = () => getPersistAdapter().get("catalog:desk-1");

describe("device binding (security review 2, M1 and M2)", () => {
  it("wipes the content cache, the push state and the pause record when another account signs in", async () => {
    await seed("user-a");
    await bindDeviceToUser("user-b");
    expect(await cached()).toBeUndefined();
    expect(await loadPushState()).toBeNull();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem(PAUSE_RECORD_KEY)).toBeNull();
    expect(localStorage.getItem(OWNER_KEY)).toBe("user-b");
  });

  it("treats an unknown owner (no marker) as a different account", async () => {
    await seed(null);
    await bindDeviceToUser("user-a");
    expect(await cached()).toBeUndefined();
  });

  it("a successful me of another account wipes the cache before its own snapshot is written", async () => {
    await seed("user-a");
    restoreApi = setApiTransport(createMockServer({ signedIn: true }).transport);
    const queryClient = new QueryClient();
    const stop = startAccountPersistence(queryClient);
    const me = await queryClient.fetchQuery(meQuery);
    await vi.waitFor(async () => expect(await cached()).toBeUndefined());
    expect(localStorage.getItem(OWNER_KEY)).toBe(me.user.id);
    await vi.waitFor(async () => expect(await getPersistAdapter().get("account:me")).toBeDefined());
    stop();
  });

  it("keeps everything for the same account, and runs once per page load", async () => {
    await seed("user-a");
    await bindDeviceToUser("user-a");
    await bindDeviceToUser("user-a");
    expect(await cached()).toEqual({ projects: ["secret prompt"] });
    expect(await loadPushState()).toMatchObject({ userId: "user-a" });
    expect(unsubscribe).not.toHaveBeenCalled();
  });

  it("drops a push subscription saved for another account even when the cache owner matches", async () => {
    await seed("user-a");
    await savePushState({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "user-z" });
    await bindDeviceToUser("user-a");
    expect(await loadPushState()).toBeNull();
    expect(await cached()).toEqual({ projects: ["secret prompt"] });
  });

  it("releaseDevice wipes everything and forgets the owner", async () => {
    await seed("user-a");
    await releaseDevice();
    expect(await cached()).toBeUndefined();
    expect(await loadPushState()).toBeNull();
    expect(localStorage.getItem(OWNER_KEY)).toBeNull();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });

  it("the sign-out path releases the device even when the logout call fails", async () => {
    await seed("user-a");
    restoreApi = setApiTransport(() => Promise.reject(new TypeError("offline")));
    await expect(signOut(new QueryClient())).rejects.toBeDefined();
    expect(await cached()).toBeUndefined();
    expect(await loadPushState()).toBeNull();
  });

  it("the guard's 401 path releases the device", async () => {
    await seed("user-a");
    restoreApi = setApiTransport(createMockServer({ signedIn: false }).transport);
    const location = { pathname: "/desktops", searchStr: "", hash: "", href: "/desktops" } as unknown as ParsedLocation;
    await expect(authGuard({ location, context: { queryClient: new QueryClient() } })).rejects.toBeDefined();
    await vi.waitFor(async () => expect(await cached()).toBeUndefined());
    expect(await loadPushState()).toBeNull();
    expect(localStorage.getItem(OWNER_KEY)).toBeNull();
  });

  it("the session expiry handler releases the device on a 401", async () => {
    await seed("user-a");
    restoreApi = setApiTransport(createMockServer({ signedIn: false }).transport);
    const router = { state: { isLoading: false, location: { pathname: "/settings", searchStr: "", hash: "" } }, navigate: vi.fn() };
    const off = installSessionExpiryHandler(new QueryClient(), router);
    const { apiFetch } = await import("./api");
    const { endpoints } = await import("./endpoints");
    await apiFetch(endpoints.passkeys).catch(() => undefined);
    await vi.waitFor(async () => expect(await cached()).toBeUndefined());
    expect(await loadPushState()).toBeNull();
    off();
  });
});
