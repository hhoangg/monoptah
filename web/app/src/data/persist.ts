// The persisted cache seam (architecture.md 3.6): the catalog and the last turns of recently opened
// sessions, so a desktop that is offline can still be read. This module holds the adapter interface, an
// in-memory default and the write and read helpers. W-12 registers the encrypted IndexedDB adapter;
// nothing here encrypts or touches storage itself. Values must be JSON-serializable and never hold keys.
import type { Turn } from "@monoptah/protocol";

export interface PersistAdapter {
  get(key: string): Promise<unknown | undefined>;
  set(key: string, value: unknown): Promise<void>;
  /** Removes everything this adapter holds (all desktops). */
  clear(): Promise<void>;
}

export class MemoryPersistAdapter implements PersistAdapter {
  readonly #map = new Map<string, unknown>();

  async get(key: string): Promise<unknown | undefined> {
    return this.#map.has(key) ? structuredClone(this.#map.get(key)) : undefined;
  }

  async set(key: string, value: unknown): Promise<void> {
    this.#map.set(key, structuredClone(value));
  }

  async clear(): Promise<void> {
    this.#map.clear();
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

/** "Forget this device" and the `revoked` state call this. Never throws: a failing store must not block them. */
export async function clearPersistedCache(): Promise<void> {
  try {
    await adapter.clear();
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
