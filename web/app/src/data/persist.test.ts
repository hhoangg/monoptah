import { afterEach, describe, expect, it } from "vitest";
import type { Turn } from "@monoptah/protocol";
import {
  MemoryPersistAdapter,
  PERSISTED_SESSION_LIMIT,
  PERSISTED_TURN_LIMIT,
  clearPersistedCache,
  getPersistAdapter,
  keyBelongsToDesktop,
  loadPersistedCatalog,
  loadPersistedTurns,
  persistCatalog,
  persistTurns,
  registerPersistAdapter,
  type PersistAdapter,
} from "./persist.ts";

const turns = (n: number): Turn[] => Array.from({ length: n }, (_, i) => ({ id: `t${i}`, items: [] }));
let original: PersistAdapter;
afterEach(() => {
  registerPersistAdapter(original ?? new MemoryPersistAdapter());
});

describe("persist adapter", () => {
  it("defaults to an in-memory adapter that stores copies and clears", async () => {
    original = getPersistAdapter();
    expect(original).toBeInstanceOf(MemoryPersistAdapter);
    const value = { a: [1] };
    await original.set("k", value);
    value.a.push(2);
    expect(await original.get("k")).toEqual({ a: [1] });
    expect(await original.get("missing")).toBeUndefined();
    await original.clear();
    expect(await original.get("k")).toBeUndefined();
  });

  it("registerPersistAdapter swaps the adapter that the helpers use", async () => {
    original = getPersistAdapter();
    const writes: string[] = [];
    const custom: PersistAdapter = {
      get: async () => undefined,
      set: async (key) => {
        writes.push(key);
      },
      clear: async () => {
        writes.push("clear");
      },
    };
    expect(registerPersistAdapter(custom)).toBe(original);
    await persistCatalog("dk1", { projects: [] });
    await clearPersistedCache();
    expect(writes).toEqual(["catalog:dk1", "clear"]);
  });

  it("keeps the last 30 turns and 10 recent sessions per desktop", async () => {
    original = getPersistAdapter();
    registerPersistAdapter(new MemoryPersistAdapter());
    await persistTurns("dk1", "s0", turns(45));
    const kept = await loadPersistedTurns("dk1", "s0");
    expect(kept).toHaveLength(PERSISTED_TURN_LIMIT);
    expect(kept?.[0]?.id).toBe("t15");
    for (let i = 1; i <= PERSISTED_SESSION_LIMIT; i += 1) await persistTurns("dk1", `s${i}`, turns(2));
    expect(await loadPersistedTurns("dk1", "s0")).toBeUndefined();
    expect(await loadPersistedTurns("dk1", "s1")).toHaveLength(2);
    expect(await loadPersistedTurns("dk2", "s1")).toBeUndefined();
  });

  it("clearPersistedCache empties the catalog and the turns and never throws", async () => {
    original = getPersistAdapter();
    registerPersistAdapter(new MemoryPersistAdapter());
    await persistCatalog("dk1", { projects: [1] });
    await persistTurns("dk1", "s1", turns(2));
    await clearPersistedCache();
    expect(await loadPersistedCatalog("dk1")).toBeUndefined();
    expect(await loadPersistedTurns("dk1", "s1")).toBeUndefined();
    registerPersistAdapter({
      get: async () => undefined,
      set: async () => undefined,
      clear: async () => {
        throw new Error("storage failed");
      },
    });
    await expect(clearPersistedCache()).resolves.toBeUndefined();
  });

  it("clearPersistedCache(desktopId) removes only that desktop (M5)", async () => {
    original = getPersistAdapter();
    registerPersistAdapter(new MemoryPersistAdapter());
    for (const id of ["dk1", "dk2"]) {
      await persistCatalog(id, { projects: [id] });
      await persistTurns(id, "s1", turns(2));
    }
    await clearPersistedCache("dk1");
    expect(await loadPersistedCatalog("dk1")).toBeUndefined();
    expect(await loadPersistedTurns("dk1", "s1")).toBeUndefined();
    expect(await loadPersistedCatalog("dk2")).toEqual({ projects: ["dk2"] });
    expect(await loadPersistedTurns("dk2", "s1")).toHaveLength(2);
    expect(keyBelongsToDesktop("turns:dk1:s1", "dk1")).toBe(true);
    expect(keyBelongsToDesktop("turns:dk10:s1", "dk1")).toBe(false);
  });
});
