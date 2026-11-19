import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const resubscribe = vi.fn((_userId: string) => Promise.resolve(false));
vi.mock("@/features/push/subscribe", () => ({ forgetPushLocally: () => Promise.resolve(), resubscribeIfVapidKeyChanged: (userId: string) => resubscribe(userId) }));
vi.mock("@/sw/pushState", () => ({ loadPushState: () => Promise.resolve({ deviceId: "dev-1", subscriptionId: "sub-1", userId: "user-a" }) }));
vi.mock("@/data", () => ({ clearPersistedCache: () => Promise.resolve() }));

import { bindDeviceToUser, OWNER_KEY, PUSH_REFRESH_MIN_INTERVAL_MS, releaseDevice, resetDeviceBindingForTests } from "./deviceBinding";

function setVisibility(state: "visible" | "hidden"): void {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-01-01T00:00:00Z"));
  resubscribe.mockClear();
  resetDeviceBindingForTests();
  localStorage.setItem(OWNER_KEY, "user-a");
});
afterEach(async () => {
  await releaseDevice();
  vi.useRealTimers();
  Reflect.deleteProperty(document, "visibilityState");
});

describe("push refresh on foreground", () => {
  it("checks once on bind, not again on every render, and again after the app comes back after a while", async () => {
    await bindDeviceToUser("user-a");
    await bindDeviceToUser("user-a");
    expect(resubscribe).toHaveBeenCalledTimes(1);

    setVisibility("visible");
    expect(resubscribe).toHaveBeenCalledTimes(1);

    vi.setSystemTime(Date.now() + PUSH_REFRESH_MIN_INTERVAL_MS + 1);
    setVisibility("hidden");
    expect(resubscribe).toHaveBeenCalledTimes(1);
    setVisibility("visible");
    expect(resubscribe).toHaveBeenCalledTimes(2);
    expect(resubscribe).toHaveBeenLastCalledWith("user-a");
  });

  it("stops checking after the session ended", async () => {
    await bindDeviceToUser("user-a");
    await releaseDevice();
    vi.setSystemTime(Date.now() + PUSH_REFRESH_MIN_INTERVAL_MS + 1);
    resubscribe.mockClear();
    setVisibility("visible");
    expect(resubscribe).not.toHaveBeenCalled();
  });

  it("a refused re-subscribe (iOS without a gesture) is swallowed and not retried inside the 60 s interval", async () => {
    resubscribe.mockImplementation(() => Promise.reject(new Error("NotAllowedError")));
    await bindDeviceToUser("user-a");
    await vi.advanceTimersByTimeAsync(0);
    for (let i = 0; i < 5; i += 1) {
      setVisibility("hidden");
      setVisibility("visible");
      await vi.advanceTimersByTimeAsync(1000);
    }
    expect(resubscribe).toHaveBeenCalledTimes(1);
    vi.setSystemTime(Date.now() + PUSH_REFRESH_MIN_INTERVAL_MS + 1);
    setVisibility("visible");
    expect(resubscribe).toHaveBeenCalledTimes(2);
    resubscribe.mockImplementation(() => Promise.resolve(false));
  });
});
