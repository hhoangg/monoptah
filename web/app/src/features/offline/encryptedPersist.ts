// Encrypted persisted cache (W-12, architecture.md 3.6): the `PersistAdapter` of data/persist.ts over
// IndexedDB.
//
// - Values are JSON-serialized, then AES-GCM encrypted with a random 96-bit IV per write. The record key
//   is the AAD, so a ciphertext copied under another key fails to decrypt.
// - The AES-GCM-256 key is created with `extractable: false` and stored in IndexedDB as a CryptoKey
//   (the same pattern as link/keys.ts): a copy of the profile files does not yield it.
// - The record key stays readable (`catalog:<desktopId>`, ...) so `clear(desktopId)` can delete one
//   desktop's entries without decrypting (the contract in data/persist.ts). Values never are.
// - `clear()` without an id wipes every record and rotates the key; `clear(desktopId)` deletes only the
//   records of that desktop.
// - All operations run one after another (a queue), so a `clear` never overtakes a `set` issued before it.
import { keyBelongsToDesktop, type PersistAdapter } from "@/data";
import { inTransaction, META_STORE, RECORDS_STORE, request } from "./idb";

export const CACHE_KEY_RECORD = "aes";
const IV_BYTES = 12;
const FORMAT = 1;

/** What is stored per record: no plaintext. */
export interface EncryptedRecord {
  v: typeof FORMAT;
  iv: Uint8Array<ArrayBuffer>;
  ciphertext: Uint8Array<ArrayBuffer>;
}

interface KeyRecord {
  v: typeof FORMAT;
  key: CryptoKey;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function newKey(): Promise<CryptoKey> {
  return crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}

/** The stored key, or a new one written with `add` semantics (a tab that lost a race uses the winner's key). */
async function loadOrCreateKey(): Promise<CryptoKey> {
  const existing = await inTransaction([META_STORE], "readonly", (tx) => request<KeyRecord | undefined>(tx.objectStore(META_STORE).get(CACHE_KEY_RECORD)));
  if (existing) return existing.key;
  const created = await newKey();
  return inTransaction([META_STORE], "readwrite", async (tx) => {
    const store = tx.objectStore(META_STORE);
    const raced = await request<KeyRecord | undefined>(store.get(CACHE_KEY_RECORD));
    if (raced) return raced.key;
    await request(store.put({ v: FORMAT, key: created } satisfies KeyRecord, CACHE_KEY_RECORD));
    return created;
  });
}

export async function encryptValue(key: CryptoKey, recordKey: string, value: unknown): Promise<EncryptedRecord> {
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const plaintext = encoder.encode(JSON.stringify(value));
  const ciphertext = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: encoder.encode(recordKey) }, key, plaintext);
  return { v: FORMAT, iv, ciphertext: new Uint8Array(ciphertext) };
}

export async function decryptValue(key: CryptoKey, recordKey: string, record: EncryptedRecord): Promise<unknown> {
  const plaintext = await crypto.subtle.decrypt({ name: "AES-GCM", iv: record.iv, additionalData: encoder.encode(recordKey) }, key, record.ciphertext);
  return JSON.parse(decoder.decode(plaintext)) as unknown;
}

export class EncryptedPersistAdapter implements PersistAdapter {
  #queue: Promise<unknown> = Promise.resolve();

  #enqueue<T>(task: () => Promise<T>): Promise<T> {
    const run = this.#queue.then(task, task);
    this.#queue = run.catch(() => undefined);
    return run;
  }

  get(key: string): Promise<unknown | undefined> {
    return this.#enqueue(async () => {
      const record = await inTransaction([RECORDS_STORE], "readonly", (tx) => request<EncryptedRecord | undefined>(tx.objectStore(RECORDS_STORE).get(key)));
      if (!record) return undefined;
      try {
        return await decryptValue(await loadOrCreateKey(), key, record);
      } catch {
        // Wrong key (another tab rotated it) or a damaged record: it is unreadable, so drop it.
        await inTransaction([RECORDS_STORE], "readwrite", (tx) => request(tx.objectStore(RECORDS_STORE).delete(key))).catch(() => undefined);
        return undefined;
      }
    });
  }

  set(key: string, value: unknown): Promise<void> {
    return this.#enqueue(async () => {
      if (value === undefined) {
        await inTransaction([RECORDS_STORE], "readwrite", (tx) => request(tx.objectStore(RECORDS_STORE).delete(key)));
        return;
      }
      const record = await encryptValue(await loadOrCreateKey(), key, value);
      await inTransaction([RECORDS_STORE], "readwrite", (tx) => request(tx.objectStore(RECORDS_STORE).put(record, key)));
    });
  }

  clear(desktopId?: string): Promise<void> {
    return this.#enqueue(async () => {
      if (desktopId === undefined) {
        // Everything goes and the key is replaced, so nothing written before could be read again even
        // if a copy of a record survived somewhere.
        const rotated = await newKey();
        await inTransaction([META_STORE, RECORDS_STORE], "readwrite", async (tx) => {
          await request(tx.objectStore(RECORDS_STORE).clear());
          await request(tx.objectStore(META_STORE).put({ v: FORMAT, key: rotated } satisfies KeyRecord, CACHE_KEY_RECORD));
        });
        return;
      }
      await inTransaction([RECORDS_STORE], "readwrite", async (tx) => {
        const store = tx.objectStore(RECORDS_STORE);
        const keys = await request(store.getAllKeys());
        for (const key of keys) if (typeof key === "string" && keyBelongsToDesktop(key, desktopId)) await request(store.delete(key));
      });
    });
  }
}

/** True where the adapter can run: IndexedDB and WebCrypto exist. */
export function encryptedPersistSupported(): boolean {
  return typeof indexedDB !== "undefined" && typeof crypto !== "undefined" && typeof crypto.subtle !== "undefined";
}
