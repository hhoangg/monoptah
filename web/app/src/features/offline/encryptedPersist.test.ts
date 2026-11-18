import "fake-indexeddb/auto";
import { IDBFactory } from "fake-indexeddb";
import { beforeEach, describe, expect, it } from "vitest";
import { CACHE_DB_NAME, META_STORE, openCacheDb, RECORDS_STORE, request } from "./idb";
import { CACHE_KEY_RECORD, EncryptedPersistAdapter, type EncryptedRecord } from "./encryptedPersist";

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
});

async function rawRecords(): Promise<Map<string, EncryptedRecord>> {
  const db = await openCacheDb();
  try {
    const tx = db.transaction([RECORDS_STORE], "readonly");
    const store = tx.objectStore(RECORDS_STORE);
    const keys = await request(store.getAllKeys());
    const values = await request(store.getAll());
    return new Map(keys.map((key, i) => [String(key), values[i] as EncryptedRecord]));
  } finally {
    db.close();
  }
}

async function rawKey(): Promise<CryptoKey | undefined> {
  const db = await openCacheDb();
  try {
    const record = await request<{ key: CryptoKey } | undefined>(db.transaction([META_STORE], "readonly").objectStore(META_STORE).get(CACHE_KEY_RECORD));
    return record?.key;
  } finally {
    db.close();
  }
}

function containsBytes(haystack: Uint8Array, needle: Uint8Array): boolean {
  outer: for (let i = 0; i + needle.length <= haystack.length; i++) {
    for (let j = 0; j < needle.length; j++) if (haystack[i + j] !== needle[j]) continue outer;
    return true;
  }
  return false;
}

describe("EncryptedPersistAdapter", () => {
  it("round-trips JSON values", async () => {
    const adapter = new EncryptedPersistAdapter();
    const value = { projects: [{ id: "p1", name: "Alpha" }], n: 3 };
    await adapter.set("catalog:d1", value);
    expect(await adapter.get("catalog:d1")).toEqual(value);
    expect(await adapter.get("catalog:other")).toBeUndefined();
  });

  it("stores no plaintext of the value, in the record or in the key blob", async () => {
    const adapter = new EncryptedPersistAdapter();
    const secret = "SECRET-TURN-TEXT-please-do-not-leak";
    await adapter.set("turns:d1:c1", [{ text: secret }]);
    const records = await rawRecords();
    const record = records.get("turns:d1:c1");
    expect(record).toBeDefined();
    const needle = new TextEncoder().encode(secret);
    expect(containsBytes(record!.ciphertext, needle)).toBe(false);
    expect(containsBytes(record!.iv, needle)).toBe(false);
    expect(JSON.stringify(Object.keys(record!))).not.toContain(secret);
    // The ciphertext is the JSON length plus the 16 byte GCM tag: it is encrypted, not just wrapped.
    expect(record!.ciphertext.length).toBe(new TextEncoder().encode(JSON.stringify([{ text: secret }])).length + 16);
  });

  it("keeps the AES key non-extractable", async () => {
    const adapter = new EncryptedPersistAdapter();
    await adapter.set("catalog:d1", { a: 1 });
    const key = await rawKey();
    expect(key?.extractable).toBe(false);
    expect(key?.algorithm).toMatchObject({ name: "AES-GCM", length: 256 });
    await expect(crypto.subtle.exportKey("raw", key!)).rejects.toThrow();
  });

  it("uses a fresh IV for every write, also for the same value", async () => {
    const adapter = new EncryptedPersistAdapter();
    await adapter.set("catalog:d1", { a: 1 });
    await adapter.set("catalog:d2", { a: 1 });
    const records = await rawRecords();
    const a = records.get("catalog:d1")!;
    const b = records.get("catalog:d2")!;
    expect(a.iv.length).toBe(12);
    expect(Array.from(a.iv)).not.toEqual(Array.from(b.iv));
    expect(Array.from(a.ciphertext)).not.toEqual(Array.from(b.ciphertext));
  });

  it("refuses a ciphertext moved under another key (the record key is the AAD)", async () => {
    const adapter = new EncryptedPersistAdapter();
    await adapter.set("catalog:d1", { a: 1 });
    const record = (await rawRecords()).get("catalog:d1")!;
    const db = await openCacheDb();
    await request(db.transaction([RECORDS_STORE], "readwrite").objectStore(RECORDS_STORE).put(record, "catalog:d2"));
    db.close();
    expect(await adapter.get("catalog:d2")).toBeUndefined();
  });

  it("clear(desktopId) removes only that desktop and keeps the key", async () => {
    const adapter = new EncryptedPersistAdapter();
    await adapter.set("catalog:d1", { a: 1 });
    await adapter.set("recent:d1", ["c1"]);
    await adapter.set("turns:d1:c1", [{ t: 1 }]);
    await adapter.set("catalog:d2", { b: 2 });
    const keyBefore = await rawKey();
    await adapter.clear("d1");
    expect([...(await rawRecords()).keys()]).toEqual(["catalog:d2"]);
    expect(await adapter.get("catalog:d2")).toEqual({ b: 2 });
    expect(await adapter.get("turns:d1:c1")).toBeUndefined();
    expect(await rawKey()).toBeDefined();
    expect(keyBefore).toBeDefined();
  });

  it("clear() wipes every record and rotates the key", async () => {
    const adapter = new EncryptedPersistAdapter();
    await adapter.set("catalog:d1", { a: 1 });
    await adapter.set("catalog:d2", { b: 2 });
    const oldRecord = (await rawRecords()).get("catalog:d1")!;
    const oldKey = await rawKey();
    await adapter.clear();
    expect((await rawRecords()).size).toBe(0);
    const newKey = await rawKey();
    expect(newKey).toBeDefined();
    expect(newKey).not.toBe(oldKey);
    // A copy of an old record can no longer be decrypted with the new key.
    await expect(
      crypto.subtle.decrypt({ name: "AES-GCM", iv: oldRecord.iv, additionalData: new TextEncoder().encode("catalog:d1") }, newKey!, oldRecord.ciphertext),
    ).rejects.toThrow();
    await adapter.set("catalog:d3", { c: 3 });
    expect(await adapter.get("catalog:d3")).toEqual({ c: 3 });
  });

  it("drops a record it cannot decrypt instead of throwing", async () => {
    const adapter = new EncryptedPersistAdapter();
    await adapter.set("catalog:d1", { a: 1 });
    const record = (await rawRecords()).get("catalog:d1")!;
    record.ciphertext = record.ciphertext.map((byte) => byte ^ 1) as EncryptedRecord["ciphertext"];
    const db = await openCacheDb();
    await request(db.transaction([RECORDS_STORE], "readwrite").objectStore(RECORDS_STORE).put(record, "catalog:d1"));
    db.close();
    expect(await adapter.get("catalog:d1")).toBeUndefined();
    expect((await rawRecords()).size).toBe(0);
  });

  it("runs a clear after the writes issued before it", async () => {
    const adapter = new EncryptedPersistAdapter();
    const writes = [adapter.set("catalog:d1", { a: 1 }), adapter.set("catalog:d2", { b: 2 })];
    const cleared = adapter.clear();
    await Promise.all([...writes, cleared]);
    expect((await rawRecords()).size).toBe(0);
  });

  it("uses its own database, not the link database", () => {
    expect(CACHE_DB_NAME).toBe("monoptah-cache");
  });
});
