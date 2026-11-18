// F-71: the emoji reactions on a Mono's messages (ported from the mobile message actions sheet). Phone-local by
// design: kept in IndexedDB in this browser (its own small database, like the seen map of F-17), per desktop
// and Mono, and never sent to the desktop. What is stored is the emoji against a message's position in the
// chat (turn id and index), never the message text.
import { useEffect, useSyncExternalStore } from "react";
import { getLinkStore } from "@/link";

export const REACTIONS_DB = "monoptah.monoReactions";
export const REACTIONS_STORE = "reactions";

/** The 12 reactions offered, two rows of six as Messages lays them out. */
export const REACTION_EMOJIS = ["\u{1F44D}", "\u{1F44E}", "❤️", "\u{1F602}", "\u{1F389}", "\u{1F62E}", "\u{1F525}", "\u{1F440}", "\u{1F64F}", "\u{1F622}", "\u{1F4AF}", "\u{1F914}"] as const;

/** emoji by message key, by Mono id. */
export type MonoReactions = Readonly<Record<string, Readonly<Record<string, string>>>>;

interface State {
  desktopId: string | null;
  loaded: boolean;
  reactions: MonoReactions;
}

let state: State = { desktopId: null, loaded: false, reactions: {} };
/** Counts the loads and resets, so a read that was overtaken by either is dropped when it comes back. */
let generation = 0;
/** A reaction was set before the stored ones were read: the merged result is written once they are. */
let dirty = false;
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

const hasIdb = () => typeof indexedDB !== "undefined";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(REACTIONS_DB, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(REACTIONS_STORE)) open.result.createObjectStore(REACTIONS_STORE);
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
      const tx = db.transaction(REACTIONS_STORE, mode);
      const request = run(tx.objectStore(REACTIONS_STORE));
      tx.oncomplete = () => resolve(request.result);
      tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
      tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
    });
  } finally {
    db.close();
  }
}

/** Keeps only string emoji under string keys, so a damaged record cannot reach the screen. */
function clean(stored: unknown): MonoReactions {
  if (!stored || typeof stored !== "object" || Array.isArray(stored)) return {};
  const out: Record<string, Record<string, string>> = {};
  for (const [monoId, byMessage] of Object.entries(stored)) {
    if (!byMessage || typeof byMessage !== "object" || Array.isArray(byMessage)) continue;
    const entries = Object.entries(byMessage).filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].length > 0 && entry[1].length <= 16);
    if (entries.length > 0) out[monoId] = Object.fromEntries(entries);
  }
  return out;
}

/** Reads the reactions of `desktopId` once; one added meanwhile wins over the stored value. */
export function loadReactions(desktopId: string | undefined): void {
  if (!desktopId || state.desktopId === desktopId) return;
  const mine = ++generation;
  dirty = false;
  publish({ desktopId, loaded: !hasIdb(), reactions: {} });
  if (!hasIdb()) return;
  void withStore<unknown>("readonly", (store) => store.get(desktopId))
    .then(clean)
    .catch(() => ({}) as MonoReactions)
    .then((stored) => {
      if (generation !== mine || state.desktopId !== desktopId) return;
      const merged: Record<string, Record<string, string>> = {};
      for (const monoId of new Set([...Object.keys(stored), ...Object.keys(state.reactions)])) merged[monoId] = { ...stored[monoId], ...state.reactions[monoId] };
      publish({ desktopId, loaded: true, reactions: merged });
      // What was set while the read was out is written on top of what was stored, never instead of it.
      if (dirty) {
        dirty = false;
        persist(desktopId, merged);
      }
    });
}

function persist(desktopId: string, reactions: MonoReactions): void {
  // A revoked link keeps nothing on disk.
  const link = getLinkStore(desktopId).getState();
  if (!hasIdb() || link.revoked || link.blocked === "revoked") return;
  void withStore("readwrite", (store) => store.put(reactions, desktopId)).catch(() => undefined);
}

function save(desktopId: string, reactions: MonoReactions): void {
  publish({ ...state, reactions });
  // Before the stored reactions are read, writing now would replace them with only this one.
  if (!state.loaded) {
    dirty = true;
    return;
  }
  persist(desktopId, reactions);
}

/** Sets (or, with null, removes) the reaction of one message of a Mono. */
export function setReaction(desktopId: string, monoId: string, messageKey: string, emoji: string | null): void {
  loadReactions(desktopId);
  const mine = { ...state.reactions[monoId] };
  if (emoji === null || mine[messageKey] === emoji) delete mine[messageKey];
  else mine[messageKey] = emoji;
  const { [monoId]: _old, ...others } = state.reactions;
  save(desktopId, Object.keys(mine).length > 0 ? { ...others, [monoId]: mine } : others);
}

/** Deletes everything kept for a desktop, in memory and in IndexedDB (forget, revoke). */
export async function forgetReactions(desktopId: string): Promise<void> {
  if (state.desktopId === desktopId) publish({ ...state, reactions: {} });
  if (!hasIdb()) return;
  try {
    await withStore("readwrite", (store) => store.delete(desktopId));
  } catch {
    // Storage blocked: nothing was kept that could be removed.
  }
}

/** Forgets what is in memory. For tests. */
export function resetReactions(): void {
  generation += 1;
  dirty = false;
  publish({ desktopId: null, loaded: false, reactions: {} });
}

const NONE: Readonly<Record<string, string>> = {};

/** The reactions of one Mono's chat, by message key. */
export function useMonoReactions(desktopId: string, monoId: string): Readonly<Record<string, string>> {
  useEffect(() => loadReactions(desktopId), [desktopId]);
  const current = useSyncExternalStore(subscribe, () => state);
  return current.desktopId === desktopId ? (current.reactions[monoId] ?? NONE) : NONE;
}

/** The key of a message: its turn and its position among the messages of that turn. */
export const messageKey = (turnId: string, index: number) => `${turnId}#${index}`;
export const parseMessageKey = (key: string): { turnId: string; index: number } | null => {
  const cut = key.lastIndexOf("#");
  const index = Number(key.slice(cut + 1));
  return cut > 0 && Number.isInteger(index) && index >= 0 ? { turnId: key.slice(0, cut), index } : null;
};
