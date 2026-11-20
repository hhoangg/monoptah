// The pending notification target. A click on a notification can reach a page that is not ready to hear a
// postMessage (a cold start from the closed app, an iOS standalone window that ignores the URL given to
// `openWindow`, a page that is still booting). So the worker also leaves the target in IndexedDB, which a worker
// and a page of the same origin both reach, and the page takes it once when it starts and when it is shown again.
//
// Shared by the worker and the page: it must stay free of app imports and of anything a worker lacks.
import { safeNotificationUrl } from "./safeUrl";

/** A target older than this is a stale tap (the app was opened by hand much later) and is dropped. */
export const PENDING_TTL_MS = 2 * 60_000;
export const PENDING_DB = "monoptah-pending-target";
export const PENDING_STORE = "pending";
export const PENDING_KEY = "target";

export interface PendingStore {
  put(url: string, at: number): Promise<void>;
  /** The saved body (see `encodePending`), removed by the read. */
  take(): Promise<string | null>;
  /** Removes the saved target only when it is for `url` (one transaction), so a newer tap is never lost. */
  clearIf(url: string): Promise<void>;
}

/** The JSON body saved for `url`, or null when the URL is not worth saving (the root has no deep target). */
export function encodePending(url: string, now: number): string | null {
  const safe = safeNotificationUrl(url);
  return safe === null ? null : JSON.stringify({ url: safe, at: now });
}

/** The URL inside a saved body, whatever its age. */
function savedUrl(raw: string): string | null {
  try {
    const value = JSON.parse(raw) as { url?: unknown };
    return typeof value.url === "string" ? value.url : null;
  } catch {
    return null;
  }
}

/** The safe, fresh URL in a saved body, else null. A clock that went backwards counts as stale. */
export function decodePending(raw: string, now: number): string | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const { url, at } = value as { url?: unknown; at?: unknown };
  if (typeof at !== "number" || now < at || now - at > PENDING_TTL_MS) return null;
  return safeNotificationUrl(url);
}

/** A store on IndexedDB, which a worker and a page share and which survives an iOS standalone cold start. Every method resolves; a storage that fails behaves as empty. */
export function idbPendingStore(factory: IDBFactory | undefined): PendingStore {
  const open = (): Promise<IDBDatabase> =>
    new Promise((resolve, reject) => {
      if (!factory) return reject(new Error("no indexedDB"));
      const request = factory.open(PENDING_DB, 1);
      request.onupgradeneeded = () => request.result.createObjectStore(PENDING_STORE);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("indexedDB open failed"));
    });
  /** Runs `work` in one readwrite transaction and resolves with its result once the transaction is complete. */
  const run = async <T>(work: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> => {
    const db = await open();
    try {
      return await new Promise<T>((resolve, reject) => {
        const tx = db.transaction(PENDING_STORE, "readwrite");
        const request = work(tx.objectStore(PENDING_STORE));
        tx.oncomplete = () => resolve(request.result);
        tx.onerror = () => reject(tx.error ?? new Error("indexedDB transaction failed"));
        tx.onabort = () => reject(tx.error ?? new Error("indexedDB transaction aborted"));
      });
    } finally {
      db.close();
    }
  };
  return {
    async put(url, at) {
      const body = encodePending(url, at);
      if (body === null) return;
      try {
        await run((store) => store.put(body, PENDING_KEY));
      } catch {
        // Private mode or a full disk: the postMessage and openWindow paths still work.
      }
    },
    async clearIf(url) {
      try {
        const db = await open();
        try {
          await new Promise<void>((resolve, reject) => {
            const tx = db.transaction(PENDING_STORE, "readwrite");
            const store = tx.objectStore(PENDING_STORE);
            const get = store.get(PENDING_KEY);
            get.onsuccess = () => {
              if (typeof get.result === "string" && savedUrl(get.result) === url) store.delete(PENDING_KEY);
            };
            tx.oncomplete = () => resolve();
            tx.onerror = () => reject(tx.error ?? new Error("indexedDB transaction failed"));
            tx.onabort = () => reject(tx.error ?? new Error("indexedDB transaction aborted"));
          });
        } finally {
          db.close();
        }
      } catch {
        // Nothing to clear.
      }
    },
    async take() {
      try {
        // One transaction reads and deletes, so two pages cannot both take it.
        const db = await open();
        try {
          const raw = await new Promise<unknown>((resolve, reject) => {
            const tx = db.transaction(PENDING_STORE, "readwrite");
            const store = tx.objectStore(PENDING_STORE);
            const get = store.get(PENDING_KEY);
            get.onsuccess = () => store.delete(PENDING_KEY);
            tx.oncomplete = () => resolve(get.result);
            tx.onerror = () => reject(tx.error ?? new Error("indexedDB transaction failed"));
            tx.onabort = () => reject(tx.error ?? new Error("indexedDB transaction aborted"));
          });
          return typeof raw === "string" ? raw : null;
        } finally {
          db.close();
        }
      } catch {
        return null;
      }
    },
  };
}

/** Takes the pending target once: the safe URL if it is fresh, else null. Either way it is gone afterwards. */
export async function takePendingTarget(store: PendingStore, now: number): Promise<string | null> {
  return (await takePendingTap(store, now))?.url ?? null;
}

/** Like `takePendingTarget`, with the tap id (the time the worker saved it) that the click message also carries. */
export async function takePendingTap(store: PendingStore, now: number): Promise<{ url: string; at: number } | null> {
  const saved = await store.take();
  if (saved === null) return null;
  const url = decodePending(saved, now);
  const at = (JSON.parse(saved) as { at: number }).at;
  return url === null ? null : { url, at };
}
