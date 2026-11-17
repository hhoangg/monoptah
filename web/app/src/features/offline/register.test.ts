import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { describe, expect, it } from "vitest";
import { clearPersistedCache, getPersistAdapter, persistCatalog, loadPersistedCatalog } from "@/data";
import { forgetDesktopLocally } from "@/features/settings/desktopLink";
import { EncryptedPersistAdapter } from "./encryptedPersist";

globalThis.indexedDB = new IDBFactory();
await import("./register");

describe("offline startup wiring", () => {
  it("registers the encrypted adapter as the persisted cache", () => {
    expect(getPersistAdapter()).toBeInstanceOf(EncryptedPersistAdapter);
  });

  it("forgetting a desktop wipes its cached entries and keeps the others", async () => {
    await persistCatalog("d1", { n: 1 });
    await persistCatalog("d2", { n: 2 });
    await forgetDesktopLocally("d1");
    expect(await loadPersistedCatalog("d1")).toBeUndefined();
    expect(await loadPersistedCatalog("d2")).toEqual({ n: 2 });
  });

  it("a revoke (clearPersistedCache with an id) and Forget this device (no id) wipe the right data", async () => {
    await persistCatalog("d1", { n: 1 });
    await persistCatalog("d2", { n: 2 });
    await clearPersistedCache("d1");
    expect(await loadPersistedCatalog("d1")).toBeUndefined();
    expect(await loadPersistedCatalog("d2")).toEqual({ n: 2 });
    await clearPersistedCache();
    expect(await loadPersistedCatalog("d2")).toBeUndefined();
  });
});
