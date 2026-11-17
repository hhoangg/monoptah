// What the service worker needs to re-subscribe on its own (`pushsubscriptionchange`): the server device id
// and the id of the stored subscription. A worker has no localStorage, so this lives in a tiny IndexedDB
// database shared with the page (features/push). Neither value is a secret.
const DB_NAME = "monoptah-push";
const STORE = "state";
const KEY = "subscription";

export interface PushState {
  deviceId: string;
  /** Server id from `POST /api/push/subscriptions`, used to delete the old row. */
  subscriptionId: string;
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(DB_NAME, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(STORE)) open.result.createObjectStore(STORE);
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    open.onblocked = () => reject(new Error("IndexedDB open blocked"));
  });
}

async function run<T>(mode: IDBTransactionMode, body: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await openDb();
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(STORE, mode);
      const req = body(tx.objectStore(STORE));
      tx.oncomplete = () => resolve(req.result);
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
      tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
    });
  } finally {
    db.close();
  }
}

export async function loadPushState(): Promise<PushState | null> {
  const value: unknown = await run("readonly", (store) => store.get(KEY));
  if (typeof value !== "object" || value === null) return null;
  const { deviceId, subscriptionId } = value as Record<string, unknown>;
  return typeof deviceId === "string" && typeof subscriptionId === "string" ? { deviceId, subscriptionId } : null;
}

export async function savePushState(state: PushState): Promise<void> {
  await run("readwrite", (store) => store.put(state, KEY));
}

export async function clearPushState(): Promise<void> {
  await run("readwrite", (store) => store.delete(KEY));
}
