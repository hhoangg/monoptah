// Tiny promise wrapper over the one IndexedDB database of the link client (security.md 5 and 6).
// A connection is opened per operation and closed right after, so a "Forget this device" or a
// test that swaps `indexedDB` never meets a stale cached connection.

export const LINK_DB_NAME = "monoptah-link";
export const LINK_DB_VERSION = 1;
/** The device key record and the non-extractable AES wrapping key. */
export const KEYS_STORE = "keys";
/** Pinned desktop public keys, keyed by desktop id. */
export const PINS_STORE = "pins";

export type LinkStore = typeof KEYS_STORE | typeof PINS_STORE;

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
  });
}

export function openLinkDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(LINK_DB_NAME, LINK_DB_VERSION);
    open.onupgradeneeded = () => {
      const db = open.result;
      for (const name of [KEYS_STORE, PINS_STORE]) {
        if (!db.objectStoreNames.contains(name)) db.createObjectStore(name);
      }
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    open.onblocked = () => reject(new Error("IndexedDB open blocked"));
  });
}

/** Runs `fn` in one transaction and resolves when the transaction has committed. */
async function run<T>(stores: LinkStore[], mode: IDBTransactionMode, fn: (tx: IDBTransaction) => Promise<T>): Promise<T> {
  const db = await openLinkDb();
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

export function idbGet<T>(store: LinkStore, key: string): Promise<T | undefined> {
  return run([store], "readonly", (tx) => request<T | undefined>(tx.objectStore(store).get(key)));
}

export async function idbPut(store: LinkStore, key: string, value: unknown): Promise<void> {
  await run([store], "readwrite", (tx) => request(tx.objectStore(store).put(value, key)));
}

export async function idbDelete(store: LinkStore, key: string): Promise<void> {
  await run([store], "readwrite", (tx) => request(tx.objectStore(store).delete(key)));
}

export async function idbClear(store: LinkStore): Promise<void> {
  await run([store], "readwrite", (tx) => request(tx.objectStore(store).clear()));
}

/** Read-modify-write of one key in a single transaction (no interleaving between read and write). */
export function idbUpdate<T, R>(store: LinkStore, key: string, fn: (current: T | undefined) => { next?: T; result: R }): Promise<R> {
  return run([store], "readwrite", async (tx) => {
    const os = tx.objectStore(store);
    const { next, result } = fn(await request<T | undefined>(os.get(key)));
    if (next !== undefined) await request(os.put(next, key));
    return result;
  });
}

/**
 * Adds every entry in ONE readwrite transaction with `add` (never overwrites). Resolves true when all
 * were written, false when any key already existed (nothing is written then).
 */
export async function idbAddAll(store: LinkStore, entries: ReadonlyArray<readonly [string, unknown]>): Promise<boolean> {
  const db = await openLinkDb();
  try {
    return await new Promise<boolean>((resolve, reject) => {
      const tx = db.transaction([store], "readwrite");
      let conflict = false;
      tx.oncomplete = () => resolve(true);
      tx.onabort = () => (conflict ? resolve(false) : reject(tx.error ?? new Error("IndexedDB transaction aborted")));
      const os = tx.objectStore(store);
      for (const [key, value] of entries) {
        const req = os.add(value, key);
        req.onerror = (event) => {
          if (req.error?.name !== "ConstraintError") return;
          conflict = true;
          event.preventDefault();
          tx.abort();
        };
      }
    });
  } finally {
    db.close();
  }
}
