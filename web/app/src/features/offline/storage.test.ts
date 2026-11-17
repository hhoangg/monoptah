import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getPersistentStorageState, requestPersistentStorage, resetPersistentStorageState } from "./storage";

function stubStorage(persist: (() => Promise<boolean>) | undefined) {
  Object.defineProperty(navigator, "storage", { value: persist ? { persist } : {}, configurable: true });
}

beforeEach(resetPersistentStorageState);
afterEach(() => {
  Reflect.deleteProperty(navigator, "storage");
});

describe("persistent storage request", () => {
  it("asks once and keeps the answer in memory", async () => {
    const persist = vi.fn().mockResolvedValue(true);
    stubStorage(persist);
    expect(getPersistentStorageState()).toBe("unknown");
    expect(await requestPersistentStorage()).toBe("granted");
    expect(await requestPersistentStorage()).toBe("granted");
    expect(persist).toHaveBeenCalledTimes(1);
    expect(getPersistentStorageState()).toBe("granted");
  });

  it("records a refusal and a failure as denied", async () => {
    stubStorage(vi.fn().mockResolvedValue(false));
    expect(await requestPersistentStorage()).toBe("denied");
    resetPersistentStorageState();
    stubStorage(vi.fn().mockRejectedValue(new Error("no")));
    expect(await requestPersistentStorage()).toBe("denied");
  });

  it("is unsupported without navigator.storage.persist", async () => {
    stubStorage(undefined);
    expect(await requestPersistentStorage()).toBe("unsupported");
  });
});
