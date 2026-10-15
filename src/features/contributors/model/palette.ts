/** Fixed avatar and chart colors, the same in light and dark. */
export const PERSON_COLORS: string[] = [
  "#7c9cff",
  "#e3a008",
  "#3fb950",
  "#f85149",
  "#d946ef",
  "#14b8a6",
  "#f97316",
  "#a78bfa",
];

export function colorFor(index: number): string {
  const n = PERSON_COLORS.length;
  return PERSON_COLORS[((index % n) + n) % n];
}

function hashIndex(personId: string): number {
  let hash = 0x811c9dc5; // FNV-1a
  for (let i = 0; i < personId.length; i += 1) {
    hash ^= personId.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash % PERSON_COLORS.length;
}

/**
 * The color of one person from a hash of their id. Two ids can collide; use
 * `personColorMap` for a set of people shown together.
 */
export function colorForPerson(personId: string): string {
  return PERSON_COLORS[hashIndex(personId)];
}

/**
 * Colors for a group of people so that no two of the first eight share one.
 * Ids are taken in sorted order; each prefers its hashed color and moves to the
 * next free one when that is taken. Once all eight are used a new round starts.
 * The same set always gives the same colors, and adding an id that sorts last
 * changes nobody else's color.
 */
export function personColorMap(personIds: string[]): Record<string, string> {
  const map: Record<string, string> = {};
  const used = new Set<number>();
  for (const id of Array.from(new Set(personIds)).sort()) {
    if (used.size === PERSON_COLORS.length) used.clear();
    let index = hashIndex(id);
    while (used.has(index)) index = (index + 1) % PERSON_COLORS.length;
    used.add(index);
    map[id] = PERSON_COLORS[index];
  }
  return map;
}

/** A person's color from the shared map, falling back to the hash for an id not in it. */
export function colorFromMap(
  colors: Record<string, string>,
  personId: string,
): string {
  return colors[personId] ?? colorForPerson(personId);
}
