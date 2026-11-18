// Per-project empty-session backgrounds (F-90), kept in IndexedDB on this device only. Own database, key
// prefix `monoptah.`; a connection is opened per operation and closed right after (as features/offline/idb.ts).
import { clampOpacity } from "./dither";

export const BACKGROUND_DB_NAME = "monoptah.backgrounds";
const STORE = "backgrounds";

export interface BackgroundRecord {
  projectId: string;
  /** Shown in settings, where the catalog may not be loaded. */
  projectName: string;
  /** The dithered, faded PNG. */
  image: Blob;
  opacity: number;
  updatedAt: number;
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(BACKGROUND_DB_NAME, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(STORE)) open.result.createObjectStore(STORE, { keyPath: "projectId" });
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    open.onblocked = () => reject(new Error("IndexedDB open blocked"));
  });
}

async function run<T>(mode: IDBTransactionMode, fn: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await openDb();
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(STORE, mode);
      const req = fn(tx.objectStore(STORE));
      tx.oncomplete = () => resolve(req.result);
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
      tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
    });
  } finally {
    db.close();
  }
}

const listeners = new Set<() => void>();

/** Calls `listener` after any write, so a settings card and an empty session stay in step. */
export function subscribeBackgrounds(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function notify() {
  for (const listener of listeners) listener();
}

export async function getBackground(projectId: string): Promise<BackgroundRecord | null> {
  const record = await run<BackgroundRecord | undefined>("readonly", (store) => store.get(projectId));
  return record ?? null;
}

export async function listBackgrounds(): Promise<BackgroundRecord[]> {
  const records = await run<BackgroundRecord[]>("readonly", (store) => store.getAll());
  return records.sort((a, b) => a.projectName.localeCompare(b.projectName));
}

export async function putBackground(record: BackgroundRecord): Promise<void> {
  await run("readwrite", (store) => store.put({ ...record, opacity: clampOpacity(record.opacity) }));
  notify();
}

export async function setBackgroundOpacity(projectId: string, opacity: number): Promise<void> {
  const record = await getBackground(projectId);
  if (!record) return;
  await putBackground({ ...record, opacity, updatedAt: Date.now() });
}

export async function removeBackground(projectId: string): Promise<void> {
  await run("readwrite", (store) => store.delete(projectId));
  notify();
}
