// Helpers for the Worker side of the DesktopRoom RPC (architecture.md 4.4). Every caller goes through these, so a
// failing room never turns an already committed D1 change into a 500.
import type { Env } from "../env.ts";
import type { RevokeTarget } from "./DesktopRoom.ts";

/** Per-isolate presence cache: `GET /api/desktops` would otherwise wake every room on each call. */
export const PRESENCE_TTL_MS = 10_000;
export const PRESENCE_CACHE_LIMIT = 1000;
const presenceCache = new Map<string, { online: boolean; expires: number }>();

/** Forgets cached presence (tests, and after this isolate revokes in a room). */
export function invalidatePresence(desktopId?: string): void {
  if (desktopId === undefined) presenceCache.clear();
  else presenceCache.delete(desktopId);
}

function cachePresence(id: string, online: boolean, now: number): void {
  presenceCache.delete(id);
  if (presenceCache.size >= PRESENCE_CACHE_LIMIT) {
    for (const [key, entry] of presenceCache) if (entry.expires <= now) presenceCache.delete(key);
    // Map iterates in insertion order: drop the oldest until there is room.
    while (presenceCache.size >= PRESENCE_CACHE_LIMIT) presenceCache.delete(presenceCache.keys().next().value as string);
  }
  presenceCache.set(id, { online, expires: now + PRESENCE_TTL_MS });
}

/** The room of one desktop. */
export function roomOf(env: Pick<Env, "ROOM">, desktopId: string) {
  return env.ROOM.get(env.ROOM.idFromName(desktopId));
}

/** Logs a failed room call with metadata only (security.md 10): the operation and the error class. */
function logFailure(operation: string, error: unknown): void {
  console.error(`room ${operation} failed: error=${error instanceof Error ? error.name : "unknown"}`);
}

/** `room.revoke(target, code)` on each desktop, in parallel, best effort (failures are logged, never thrown). */
export async function revokeInRooms(env: Pick<Env, "ROOM">, desktopIds: readonly string[], target: RevokeTarget, code: number): Promise<void> {
  const settled = await Promise.allSettled(desktopIds.map((id) => roomOf(env, id).revoke(target, code)));
  for (const result of settled) if (result.status === "rejected") logFailure("revoke", result.reason);
  for (const id of desktopIds) invalidatePresence(id);
}

/** `room.notifyGrants()` on one desktop, best effort. */
export async function notifyGrants(env: Pick<Env, "ROOM">, desktopId: string): Promise<void> {
  try {
    await roomOf(env, desktopId).notifyGrants();
  } catch (error) {
    logFailure("notifyGrants", error);
  }
}

/** `room.setGrantState(deviceId, state)`: refreshes the grant state kept on the device's sockets, best effort. */
export async function setGrantStateInRooms(env: Pick<Env, "ROOM">, desktopId: string, deviceId: string, state: "pending" | "confirmed"): Promise<void> {
  try {
    await roomOf(env, desktopId).setGrantState(deviceId, state);
  } catch (error) {
    logFailure("setGrantState", error);
  }
}

/** `room.presence()` per desktop in parallel (cached 10 s per isolate); a failing room counts as offline and is not cached. */
export async function presenceOf(env: Pick<Env, "ROOM">, desktopIds: readonly string[]): Promise<Map<string, boolean>> {
  const now = Date.now();
  const online = new Map<string, boolean>();
  const missing: string[] = [];
  for (const id of desktopIds) {
    const hit = presenceCache.get(id);
    if (hit && hit.expires > now) online.set(id, hit.online);
    else missing.push(id);
  }
  const settled = await Promise.allSettled(missing.map((id) => roomOf(env, id).presence()));
  settled.forEach((result, i) => {
    const id = missing[i] as string;
    if (result.status === "fulfilled") {
      online.set(id, result.value.online);
      cachePresence(id, result.value.online, now);
    } else {
      logFailure("presence", result.reason);
      online.set(id, false);
    }
  });
  return online;
}
