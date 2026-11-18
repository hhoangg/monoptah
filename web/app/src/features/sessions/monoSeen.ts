// F-17: which Mono replies this browser has seen (ported from mobile/src/features/workspace/monoSeen.ts).
// For each desktop it keeps a Mono's activity as of when the user last saw it, so a reply that came after
// reads as new. Phone-local: stored in IndexedDB on this device (its own small database, as the link's
// database has a fixed set of stores) and never synced; the desktop sends no count of its own.
import { useEffect, useMemo, useSyncExternalStore } from "react";
import { useCatalog } from "@/data";
import type { CatalogMono } from "./workspaceFilter";

export const MONO_SEEN_DB = "monoptah-mono-seen";
export const MONO_SEEN_STORE = "seen";

type Seen = Readonly<Record<string, string>>;
type UnreadSource = Pick<CatalogMono, "id" | "status" | "unread" | "lastMessage" | "lastActiveAt">;

interface State {
  desktopId: string | null;
  /** False until the stored map of `desktopId` has been read. */
  loaded: boolean;
  seen: Seen;
}

let state: State = { desktopId: null, loaded: false, seen: {} };
const listeners = new Set<() => void>();

function publish(next: State): void {
  state = next;
  listeners.forEach((listener) => listener());
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(MONO_SEEN_DB, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(MONO_SEEN_STORE)) open.result.createObjectStore(MONO_SEEN_STORE);
    };
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    open.onblocked = () => reject(new Error("IndexedDB open blocked"));
  });
}

async function withStore<T>(mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await openDb();
  try {
    return await new Promise<T>((resolve, reject) => {
      const tx = db.transaction(MONO_SEEN_STORE, mode);
      const request = run(tx.objectStore(MONO_SEEN_STORE));
      tx.oncomplete = () => resolve(request.result);
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
      tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
    });
  } finally {
    db.close();
  }
}

const hasIdb = () => typeof indexedDB !== "undefined";

/** Reads the map of `desktopId` once; a Mono marked meanwhile wins over the stored value. */
export function loadMonoSeen(desktopId: string | undefined): void {
  if (!desktopId || state.desktopId === desktopId) return;
  publish({ desktopId, loaded: !hasIdb(), seen: {} });
  if (!hasIdb()) return;
  void withStore<unknown>("readonly", (store) => store.get(desktopId))
    .then((stored) => (stored && typeof stored === "object" ? (stored as Seen) : {}))
    .catch(() => ({}) as Seen)
    .then((stored) => {
      if (state.desktopId !== desktopId) return;
      publish({ desktopId, loaded: true, seen: { ...stored, ...state.seen } });
    });
}

function save(desktopId: string, seen: Seen): void {
  publish({ ...state, seen });
  if (!hasIdb()) return;
  // Storage blocked or full: the map lasts until the page reloads.
  void withStore("readwrite", (store) => store.put(seen, desktopId)).catch(() => undefined);
}

/** A Mono's last reply as far as the browser can tell; null when it cannot. */
function activity(mono: UnreadSource): string | null {
  return mono.lastMessage || mono.lastActiveAt ? `${mono.lastActiveAt}\n${mono.lastMessage}` : null;
}

/** Marks the Mono's latest as seen, as its chat shows it. */
export function markMonoSeen(desktopId: string | undefined, mono: UnreadSource): void {
  loadMonoSeen(desktopId);
  const now = activity(mono);
  if (desktopId && now !== null && state.seen[mono.id] !== now) save(desktopId, { ...state.seen, [mono.id]: now });
}

/** Marks the Mono's latest as not seen, so it reads as new until opened again. */
export function markMonoUnread(desktopId: string | undefined, mono: UnreadSource): void {
  loadMonoSeen(desktopId);
  if (desktopId && state.seen[mono.id] !== "") save(desktopId, { ...state.seen, [mono.id]: "" });
}

/** Forgets what is in memory. For tests. */
export function resetMonoSeen(): void {
  publish({ desktopId: null, loaded: false, seen: {} });
}

/**
 * The Monos with a reply the user has not seen. One seen for the first time counts as seen, so a new
 * browser does not open to every Mono marked; one still working has nothing finished to show yet; one the
 * desktop counts unread is unread.
 */
export function useUnreadMonos(desktopId: string | undefined, monos: readonly UnreadSource[]): Set<string> {
  useEffect(() => loadMonoSeen(desktopId), [desktopId]);
  const current = useSyncExternalStore(subscribe, () => state);
  const ready = current.desktopId === desktopId && current.loaded;
  useEffect(() => {
    if (!desktopId || !ready) return;
    const fresh = monos.filter((mono) => !(mono.id in state.seen) && activity(mono) !== null);
    if (fresh.length > 0) save(desktopId, { ...state.seen, ...Object.fromEntries(fresh.map((mono) => [mono.id, activity(mono) as string])) });
  }, [desktopId, ready, monos]);
  return useMemo(
    () =>
      new Set(
        monos
          .filter((mono) => {
            if (mono.status === "working") return false;
            if ((mono.unread ?? 0) > 0) return true;
            const now = activity(mono);
            return ready && now !== null && mono.id in current.seen && current.seen[mono.id] !== now;
          })
          .map((mono) => mono.id),
      ),
    [monos, current, ready],
  );
}

const NO_MONOS: readonly UnreadSource[] = [];

/** The unread Monos of the nearest desktop's catalog. */
export function useCatalogUnreadMonos(desktopId: string): Set<string> {
  const monos = useCatalog().data?.monos;
  return useUnreadMonos(desktopId, monos ?? NO_MONOS);
}
