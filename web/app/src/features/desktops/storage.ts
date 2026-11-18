// Small localStorage records of the desktop screens. Owner: WS-M (W-04, U-01). Keys use the `monoptah.`
// prefix; nothing sensitive is stored (desktop and device ids, a flag, an order).

const GRANT_PREFIX = "monoptah.desktops.grantRequested.";
const LAST_USED_KEY = "monoptah.desktops.lastUsed";
const LAST_USED_MAX = 50;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private mode or quota: the marker is only an optimisation, the grant request is idempotent.
  }
}

function grantKey(desktopId: string, deviceId: string): string {
  return `${GRANT_PREFIX}${desktopId}.${deviceId}`;
}

/** True when this device already asked for access to this desktop (so later opens skip the POST). */
export function wasGrantRequested(desktopId: string, deviceId: string): boolean {
  return read(grantKey(desktopId, deviceId)) === "1";
}

export function markGrantRequested(desktopId: string, deviceId: string): void {
  write(grantKey(desktopId, deviceId), "1");
}

/** Desktop ids, most recently opened first. */
export function readLastUsed(): string[] {
  const raw = read(LAST_USED_KEY);
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}

/** Moves a desktop to the front of the last-used order. */
export function touchLastUsed(desktopId: string): void {
  const next = [desktopId, ...readLastUsed().filter((id) => id !== desktopId)].slice(0, LAST_USED_MAX);
  write(LAST_USED_KEY, JSON.stringify(next));
}

/** Sorts by last use; desktops never opened keep their given order after the used ones. */
export function sortByLastUsed<T extends { id: string }>(items: readonly T[], order: readonly string[]): T[] {
  const rank = (id: string): number => {
    const index = order.indexOf(id);
    return index === -1 ? Number.MAX_SAFE_INTEGER : index;
  };
  return items
    .map((item, position) => ({ item, position }))
    .sort((a, b) => rank(a.item.id) - rank(b.item.id) || a.position - b.position)
    .map(({ item }) => item);
}
