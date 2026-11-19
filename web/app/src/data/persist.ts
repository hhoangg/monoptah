// The persisted cache seam (architecture.md 3.6): the catalog and the last turns of recently opened
// sessions, so a desktop that is offline can still be read. This module holds the adapter interface, an
// in-memory default and the write and read helpers. W-12 registers the encrypted IndexedDB adapter;
// nothing here encrypts or touches storage itself. Values must be JSON-serializable and never hold keys.
import type { Turn } from "@monoptah/protocol";

export interface PersistAdapter {
  get(key: string): Promise<unknown | undefined>;
  set(key: string, value: unknown): Promise<void>;
  /**
   * Removes what this adapter holds: with a `desktopId` only the entries of that desktop (a revoke of one
   * desktop must not wipe the others), without it everything ("Forget this device").
   *
   * Contract for W-12: an entry belongs to a desktop when `keyBelongsToDesktop(key, desktopId)` is true,
   * that is the second `:` separated segment of its key (`catalog:<id>`, `recent:<id>`,
   * `turns:<id>:<conversation>`). An encrypted store keeps that segment readable (or indexes on it) so it
   * can delete by desktop without decrypting.
   */
  clear(desktopId?: string): Promise<void>;
}

/** True when the persist key (`catalog:<id>`, `recent:<id>`, `turns:<id>:<conversation>`) belongs to the desktop. */
export function keyBelongsToDesktop(key: string, desktopId: string): boolean {
  return key.split(":")[1] === desktopId;
}

export class MemoryPersistAdapter implements PersistAdapter {
  readonly #map = new Map<string, unknown>();

  async get(key: string): Promise<unknown | undefined> {
    return this.#map.has(key) ? structuredClone(this.#map.get(key)) : undefined;
  }

  async set(key: string, value: unknown): Promise<void> {
    this.#map.set(key, structuredClone(value));
  }

  async clear(desktopId?: string): Promise<void> {
    if (desktopId === undefined) {
      this.#map.clear();
      return;
    }
    for (const key of [...this.#map.keys()]) if (keyBelongsToDesktop(key, desktopId)) this.#map.delete(key);
  }
}

let adapter: PersistAdapter = new MemoryPersistAdapter();

/** Replaces the adapter (W-12 registers the encrypted IndexedDB one). Returns the previous one. */
export function registerPersistAdapter(next: PersistAdapter): PersistAdapter {
  const previous = adapter;
  adapter = next;
  return previous;
}

export function getPersistAdapter(): PersistAdapter {
  return adapter;
}

/**
 * "Forget this device" (no argument: every desktop) and the `revoked` state of one desktop (with its id)
 * call this. Never throws: a failing store must not block them.
 */
export async function clearPersistedCache(desktopId?: string): Promise<void> {
  try {
    await (desktopId === undefined ? adapter.clear() : adapter.clear(desktopId));
  } catch {
    // Nothing more to do; the caller is already leaving the desktop.
  }
}

/** Turns kept per session (architecture.md 3.6). */
export const PERSISTED_TURN_LIMIT = 30;
/** Sessions kept per desktop. */
export const PERSISTED_SESSION_LIMIT = 10;

const catalogKey = (desktopId: string) => `catalog:${desktopId}`;
const turnsKey = (desktopId: string, conversationId: string) => `turns:${desktopId}:${conversationId}`;
const recentKey = (desktopId: string) => `recent:${desktopId}`;

export async function persistCatalog(desktopId: string, catalog: unknown): Promise<void> {
  try {
    await adapter.set(catalogKey(desktopId), catalog);
  } catch {
    // A failed write is only a missed cache entry.
  }
}

export async function loadPersistedCatalog(desktopId: string): Promise<unknown | undefined> {
  try {
    return await adapter.get(catalogKey(desktopId));
  } catch {
    return undefined;
  }
}

/** Keeps the last `PERSISTED_TURN_LIMIT` turns of a session and remembers it among the recent ones. */
export async function persistTurns(desktopId: string, conversationId: string, turns: Turn[]): Promise<void> {
  try {
    await adapter.set(turnsKey(desktopId, conversationId), turns.slice(-PERSISTED_TURN_LIMIT));
    const previous = await adapter.get(recentKey(desktopId));
    const recent = Array.isArray(previous) ? previous.filter((id): id is string => typeof id === "string") : [];
    const next = [conversationId, ...recent.filter((id) => id !== conversationId)];
    for (const dropped of next.slice(PERSISTED_SESSION_LIMIT)) await adapter.set(turnsKey(desktopId, dropped), []);
    await adapter.set(recentKey(desktopId), next.slice(0, PERSISTED_SESSION_LIMIT));
  } catch {
    // A failed write is only a missed cache entry.
  }
}

export async function loadPersistedTurns(desktopId: string, conversationId: string): Promise<Turn[] | undefined> {
  try {
    const value = await adapter.get(turnsKey(desktopId, conversationId));
    return Array.isArray(value) && value.length > 0 ? (value as Turn[]) : undefined;
  } catch {
    return undefined;
  }
}
