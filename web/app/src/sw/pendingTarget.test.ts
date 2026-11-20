import { IDBFactory } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import { PENDING_TTL_MS, decodePending, encodePending, idbPendingStore, takePendingTarget, type PendingStore } from "./pendingTarget";

function memoryStore(initial: { url: string; at: number } | null): PendingStore {
  let value = initial ? JSON.stringify(initial) : null;
  return {
    put: (url, at) => Promise.resolve(void (value = encodePending(url, at))),
    take: () => {
      const taken = value;
      value = null;
      return Promise.resolve(taken);
    },
    clearIf: () => Promise.resolve(),
  };
}

describe("encodePending and decodePending", () => {
  it("round-trips a Mono chat and a session URL", () => {
    for (const url of ["/d/dk1/monos/m7", "/d/dk2/s/s9?x=1#top"]) {
      expect(decodePending(encodePending(url, 1000)!, 1500)).toBe(url);
    }
  });

  it("does not save the root, an unsafe URL or a non-string", () => {
    expect(encodePending("/", 1)).toBeNull();
    expect(encodePending("https://evil.example/d/x", 1)).toBeNull();
    expect(encodePending("//evil.example", 1)).toBeNull();
  });

  it("drops a stale entry, a clock that went back and a hostile or broken body", () => {
    const body = encodePending("/d/dk1/s/s9", 1000)!;
    expect(decodePending(body, 1000 + PENDING_TTL_MS)).toBe("/d/dk1/s/s9");
    expect(decodePending(body, 1001 + PENDING_TTL_MS)).toBeNull();
    expect(decodePending(body, 999)).toBeNull();
    expect(decodePending(JSON.stringify({ url: "//evil.example", at: 1000 }), 1000)).toBeNull();
    expect(decodePending("not json", 1000)).toBeNull();
    expect(decodePending("null", 1000)).toBeNull();
  });
});

describe("idbPendingStore", () => {
  it("returns a saved target once", async () => {
    const store = idbPendingStore(new IDBFactory());
    await store.put("/d/dk1/monos/m7", 5000);
    await expect(takePendingTarget(store, 6000)).resolves.toBe("/d/dk1/monos/m7");
    await expect(takePendingTarget(store, 6000)).resolves.toBeNull();
  });

  it("keeps only the latest tap", async () => {
    const store = idbPendingStore(new IDBFactory());
    await store.put("/d/dk1/s/s9", 5000);
    await store.put("/d/dk2/monos/m7", 5100);
    await expect(takePendingTarget(store, 5200)).resolves.toBe("/d/dk2/monos/m7");
  });

  it("clearIf removes only a target for the same URL", async () => {
    const store = idbPendingStore(new IDBFactory());
    await store.put("/d/dk2/monos/m7", 5000);
    await store.clearIf("/d/dk1/s/s9");
    await expect(takePendingTarget(store, 5100)).resolves.toBe("/d/dk2/monos/m7");
    await store.put("/d/dk2/monos/m7", 5000);
    await store.clearIf("/d/dk2/monos/m7");
    await expect(store.take()).resolves.toBeNull();
  });

  it("is empty without IndexedDB and never throws", async () => {
    const store = idbPendingStore(undefined);
    await store.put("/d/dk1/s/s9", 1);
    await expect(store.take()).resolves.toBeNull();
  });

  it("ignores the root and a stale entry", async () => {
    const store = idbPendingStore(new IDBFactory());
    await store.put("/", 1);
    await expect(store.take()).resolves.toBeNull();
    await store.put("/d/dk1/s/s9", 1);
    await expect(takePendingTarget(store, 1 + PENDING_TTL_MS + 1)).resolves.toBeNull();
  });
});

describe("takePendingTarget", () => {
  it("consumes a stale entry without returning it", async () => {
    const store = memoryStore({ url: "/d/dk1/s/s9", at: 0 });
    await expect(takePendingTarget(store, PENDING_TTL_MS + 1)).resolves.toBeNull();
    await expect(store.take()).resolves.toBeNull();
  });
});
