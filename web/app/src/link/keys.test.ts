import { IDBFactory } from "fake-indexeddb";
import { bytesToHex } from "@noble/hashes/utils.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as Keys from "./keys.ts";
import { KEYS_STORE, LINK_DB_NAME, PINS_STORE, idbAddAll, idbGet, idbPut } from "./idb.ts";



async function freshModule(): Promise<typeof Keys> {
  vi.resetModules();
  return import("./keys.ts");
}

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  localStorage.clear();
  sessionStorage.clear();
});

function toHexAndBase64(bytes: Uint8Array): string[] {
  return [bytesToHex(bytes), Buffer.from(bytes).toString("base64"), Buffer.from(bytes).toString("base64url")];
}

function containsBytes(haystack: Uint8Array, needle: Uint8Array): boolean {
  outer: for (let i = 0; i + needle.length <= haystack.length; i++) {
    for (let j = 0; j < needle.length; j++) if (haystack[i + j] !== needle[j]) continue outer;
    return true;
  }
  return false;
}

/** Collects every byte sequence and string reachable from a value (CryptoKey is opaque and skipped). */
function collect(value: unknown, bytes: Uint8Array[], strings: string[], seen = new Set<unknown>()): void {
  if (typeof value === "string") strings.push(value);
  else if (value instanceof ArrayBuffer) bytes.push(new Uint8Array(value));
  else if (ArrayBuffer.isView(value)) bytes.push(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
  else if (value && typeof value === "object" && !(value instanceof CryptoKey) && !seen.has(value)) {
    seen.add(value);
    for (const v of Object.values(value)) collect(v, bytes, strings, seen);
  }
}

async function walkDatabase(): Promise<{ records: unknown[]; stores: string[] }> {
  const db = await new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open(LINK_DB_NAME);
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
  const stores = Array.from(db.objectStoreNames);
  const records: unknown[] = [];
  for (const name of stores) {
    const all = await new Promise<unknown[]>((resolve, reject) => {
      const req = db.transaction(name).objectStore(name).getAll();
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    records.push(...all);
  }
  db.close();
  return { records, stores };
}

describe("device key storage", () => {
  it("creates a 32-byte X25519 pair and survives a reload (new module instance)", async () => {
    const first = await freshModule();
    const pair = await first.getDeviceKeyPair();
    const privateCopy = pair.privateKey.slice();
    const publicCopy = pair.publicKey.slice();
    expect(privateCopy).toHaveLength(32);
    first.lockDeviceKey();

    const second = await freshModule();
    const again = await second.getDeviceKeyPair();
    expect(bytesToHex(again.publicKey)).toBe(bytesToHex(publicCopy));
    expect(bytesToHex(again.privateKey)).toBe(bytesToHex(privateCopy));
    expect(bytesToHex((await second.loadOrCreateDeviceKey()).publicKey)).toBe(bytesToHex(publicCopy));
  });

  it("never writes the private key in clear and keeps the AES key non-extractable", async () => {
    const keys = await freshModule();
    const pair = await keys.getDeviceKeyPair();
    const secret = pair.privateKey.slice();
    await keys.bindDeviceId("0123456789abcdef0123456789abcdef", pair.publicKey);
    await keys.getDeviceKeyPair();

    const { records, stores } = await walkDatabase();
    expect(stores.sort()).toEqual([KEYS_STORE, PINS_STORE].sort());
    expect(records.length).toBeGreaterThanOrEqual(2);
    const bytes: Uint8Array[] = [];
    const strings: string[] = [];
    collect(records, bytes, strings);
    for (const chunk of bytes) expect(containsBytes(chunk, secret)).toBe(false);
    const forms = toHexAndBase64(secret);
    for (const text of strings) for (const form of forms) expect(text.includes(form)).toBe(false);
    for (const storage of [localStorage, sessionStorage]) {
      for (let i = 0; i < storage.length; i++) {
        const value = storage.getItem(storage.key(i)!) ?? "";
        for (const form of forms) expect(value.includes(form)).toBe(false);
      }
    }

    const wrapping = (await idbGet<{ key: CryptoKey }>(KEYS_STORE, keys.WRAPPING_KEY_RECORD))!.key;
    expect(wrapping).toBeInstanceOf(CryptoKey);
    expect(wrapping.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("raw", wrapping)).rejects.toThrow();
  });

  it("fails to unwrap a ciphertext copied under another key id (AAD binding)", async () => {
    const keys = await freshModule();
    await keys.getDeviceKeyPair();
    keys.lockDeviceKey();
    const record = (await idbGet<Keys.DeviceKeyRecord>(KEYS_STORE, keys.DEVICE_KEY_RECORD))!;
    await idbPut(KEYS_STORE, keys.DEVICE_KEY_RECORD, { ...record, keyId: "another-key-id" });
    await expect(keys.getDeviceKeyPair()).rejects.toThrow("cannot be unlocked");
  });

  it("zeroes the in-memory key on pagehide and unwraps again afterwards", async () => {
    const keys = await freshModule();
    const pair = await keys.getDeviceKeyPair();
    const expected = pair.privateKey.slice();
    expect(pair.privateKey.some((b) => b !== 0)).toBe(true);
    window.dispatchEvent(new Event("pagehide"));
    expect(pair.privateKey.every((b) => b === 0)).toBe(true);
    const again = await keys.getDeviceKeyPair();
    expect(again.privateKey).not.toBe(pair.privateKey);
    expect(bytesToHex(again.privateKey)).toBe(bytesToHex(expected));
  });

  it("binds the server device id without changing the key", async () => {
    const keys = await freshModule();
    const created = await keys.loadOrCreateDeviceKey();
    expect(created.deviceId).toBeUndefined();
    await keys.bindDeviceId("abcdefabcdefabcdefabcdefabcdefab", created.publicKey);
    keys.lockDeviceKey();
    const loaded = await keys.loadOrCreateDeviceKey();
    expect(loaded.deviceId).toBe("abcdefabcdefabcdefabcdefabcdefab");
    expect(bytesToHex(loaded.publicKey)).toBe(bytesToHex(created.publicKey));
    expect(loaded.keyId).toBe(created.keyId);
  });

  it("forgetDeviceKey deletes the records, and the next use creates a new key", async () => {
    const keys = await freshModule();
    const before = await keys.loadOrCreateDeviceKey();
    await keys.forgetDeviceKey();
    expect(await keys.getDeviceKeyInfo()).toBeNull();
    expect(await idbGet(KEYS_STORE, keys.WRAPPING_KEY_RECORD)).toBeUndefined();
    const after = await keys.loadOrCreateDeviceKey();
    expect(bytesToHex(after.publicKey)).not.toBe(bytesToHex(before.publicKey));
  });

  it("concurrent first use creates one key", async () => {
    const keys = await freshModule();
    const [a, b] = await Promise.all([keys.getDeviceKeyPair(), keys.getDeviceKeyPair()]);
    expect(bytesToHex(a.publicKey)).toBe(bytesToHex(b.publicKey));
  });

  it("requestPersistentStorage returns the boolean and never throws", async () => {
    const keys = await freshModule();
    const original = Object.getOwnPropertyDescriptor(navigator, "storage");
    Object.defineProperty(navigator, "storage", { value: undefined, configurable: true });
    expect(await keys.requestPersistentStorage()).toBe(false);
    Object.defineProperty(navigator, "storage", { value: { persist: async () => true }, configurable: true });
    expect(await keys.requestPersistentStorage()).toBe(true);
    Object.defineProperty(navigator, "storage", { value: { persist: async () => { throw new Error("denied"); } }, configurable: true });
    expect(await keys.requestPersistentStorage()).toBe(false);
    if (original) Object.defineProperty(navigator, "storage", original);
    else Reflect.deleteProperty(navigator, "storage");
  });
});

/** A Web Locks stand-in: one holder at a time per name, in request order. */
function installFakeLocks(): { names: string[]; maxActive: () => number } {
  const names: string[] = [];
  let active = 0;
  let max = 0;
  let chain: Promise<unknown> = Promise.resolve();
  const locks = {
    request: <R,>(name: string, callback: () => Promise<R>): Promise<R> => {
      names.push(name);
      const run = chain.then(async () => {
        active += 1;
        max = Math.max(max, active);
        try {
          return await callback();
        } finally {
          active -= 1;
        }
      });
      chain = run.catch(() => undefined);
      return run;
    },
  };
  Object.defineProperty(navigator, "locks", { value: locks, configurable: true });
  return { names, maxActive: () => max };
}

describe("two tabs share one IndexedDB (M1, M2)", () => {
  afterEach(() => {
    Reflect.deleteProperty(navigator, "locks");
    vi.restoreAllMocks();
  });

  async function twoTabs(): Promise<[typeof Keys, typeof Keys]> {
    const a = await freshModule();
    const b = await freshModule();
    return [a, b];
  }

  it("without Web Locks, two first uses converge on one stored key that survives a reload", async () => {
    // Tab A's AES-GCM encrypt is slow, as on a busy thread pool: the old code let tab B's wrapping key win and brick A's record.
    const real = crypto.subtle.encrypt.bind(crypto.subtle);
    let calls = 0;
    vi.spyOn(crypto.subtle, "encrypt").mockImplementation(async (...args: Parameters<SubtleCrypto["encrypt"]>) => {
      const slow = calls++ === 0;
      const out = await real(...args);
      if (slow) await new Promise((resolve) => setTimeout(resolve, 50));
      return out;
    });
    const [a, b] = await twoTabs();
    const [pa, pb] = await Promise.all([a.getDeviceKeyPair(), b.getDeviceKeyPair()]);
    expect(bytesToHex(pa.publicKey)).toBe(bytesToHex(pb.publicKey));
    const reloaded = await freshModule();
    const again = await reloaded.getDeviceKeyPair();
    expect(bytesToHex(again.publicKey)).toBe(bytesToHex(pa.publicKey));
    expect(bytesToHex(again.privateKey)).toBe(bytesToHex(pa.privateKey));
  });

  it("with Web Locks, creation is serialised under the monoptah-device-key lock", async () => {
    const locks = installFakeLocks();
    const [a, b] = await twoTabs();
    const [pa, pb] = await Promise.all([a.getDeviceKeyPair(), b.getDeviceKeyPair()]);
    expect(bytesToHex(pa.publicKey)).toBe(bytesToHex(pb.publicKey));
    expect(locks.names).toEqual(["monoptah-device-key", "monoptah-device-key"]);
    expect(locks.maxActive()).toBe(1);
  });

  it("idbAddAll writes every entry or none", async () => {
    await idbPut(KEYS_STORE, "b", 2);
    expect(await idbAddAll(KEYS_STORE, [["a", 1], ["b", 3]])).toBe(false);
    expect(await idbGet(KEYS_STORE, "a")).toBeUndefined();
    expect(await idbGet(KEYS_STORE, "b")).toBe(2);
    expect(await idbAddAll(KEYS_STORE, [["a", 1], ["c", 3]])).toBe(true);
    expect(await idbGet(KEYS_STORE, "c")).toBe(3);
  });

  it("bindDeviceId refuses a key that is not the stored one and leaves the record alone", async () => {
    const keys = await freshModule();
    const stored = await keys.loadOrCreateDeviceKey();
    const other = new Uint8Array(32).fill(5);
    await expect(keys.bindDeviceId("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", other)).rejects.toMatchObject({ name: "DeviceKeyError", code: "mismatch" });
    expect((await keys.getDeviceKeyInfo())?.deviceId).toBeUndefined();
    await keys.bindDeviceId("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", stored.publicKey);
    expect((await keys.getDeviceKeyInfo())?.deviceId).toBe("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
  });
});

describe("pagehide while the key loads (L6)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("a lock before the first read wins over the load", async () => {
    const keys = await freshModule();
    const load = keys.getDeviceKeyPair();
    keys.lockDeviceKey();
    await expect(load).rejects.toMatchObject({ name: "DeviceKeyError", code: "locked" });
    await expect(keys.getDeviceKeyPair()).resolves.toMatchObject({ publicKey: expect.any(Uint8Array) });
  });

  it("a pagehide during the unwrap does not leave the key in memory", async () => {
    const keys = await freshModule();
    const first = await keys.getDeviceKeyPair();
    const expected = bytesToHex(first.publicKey);
    keys.lockDeviceKey();
    const real = crypto.subtle.decrypt.bind(crypto.subtle);
    vi.spyOn(crypto.subtle, "decrypt").mockImplementationOnce(async (...args: Parameters<SubtleCrypto["decrypt"]>) => {
      window.dispatchEvent(new Event("pagehide"));
      return real(...args);
    });
    await expect(keys.getDeviceKeyPair()).rejects.toMatchObject({ code: "locked" });
    // The next use unwraps again and gets the same key.
    expect(bytesToHex((await keys.getDeviceKeyPair()).publicKey)).toBe(expected);
  });
});
