// Promise wrapper over the IndexedDB database of the encrypted persisted cache (architecture.md 3.6).
// Separate from the link database (`monoptah-link`, WS-L): this one holds the cache key and the cache
// records only. A connection is opened per operation and closed right after, so a test that swaps
// `indexedDB` never meets a stale connection.

export const CACHE_DB_NAME = "monoptah-cache";
export const CACHE_DB_VERSION = 1;
/** One record: the non-extractable AES-GCM key of the cache. */
export const META_STORE = "meta";
/** Encrypted records, keyed by the persist key (`catalog:<desktopId>`, ...). */
export const RECORDS_STORE = "records";

export type CacheStoreName = typeof META_STORE | typeof RECORDS_STORE;

export function openCacheDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(CACHE_DB_NAME, CACHE_DB_VERSION);
    open.onupgradeneeded = () => {
      const db = open.result;
      for (const name of [META_STORE, RECORDS_STORE]) {
        if (!db.objectStoreNames.contains(name)) db.createObjectStore(name);
      }
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    open.onblocked = () => reject(new Error("IndexedDB open blocked"));
  });
}

export function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
  });
}

/** Runs `fn` in one transaction over `stores` and resolves once the transaction has committed. */
export async function inTransaction<T>(
  stores: CacheStoreName[],
  mode: IDBTransactionMode,
  fn: (tx: IDBTransaction) => Promise<T>,
): Promise<T> {
  const db = await openCacheDb();
  try {
    const tx = db.transaction(stores, mode);
    const done = new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
      tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
    });
    const value = await fn(tx);
    await done;
    return value;
  } finally {
    db.close();
  }
}
