import { PERIOD_KINDS, periodFromId } from "./period";
import type { PeriodKind, PeriodPick } from "./period";

// The `monocode.` prefix is deliberately kept: renaming it would drop every
// saved preference on existing installs.
export const RANGE_KEY = "monocode.contributors.range";
export const MERGE_KEY = "monocode.contributors.mergeIdentities";

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // private mode / quota: the control still works for this window
  }
}

const isKind = (text: string): text is PeriodKind =>
  PERIOD_KINDS.some((k) => k.value === text);

/** What a saved preference holds: `<kind>` (follows the current period) or `<kind>:<id>`. */
export function serializePick(pick: PeriodPick): string {
  return pick.id === null ? pick.kind : `${pick.kind}:${pick.id}`;
}

/**
 * The pick a saved text stands for, or undefined when it is not one. That
 * covers the removed ranges (`30d`, `custom:...`, `90d`, `1y`, `all`), a
 * pinned id that does not parse, and one that starts after today.
 */
export function parsePick(
  value: string | null | undefined,
  now: Date,
): PeriodPick | undefined {
  if (!value) return undefined;
  const at = value.indexOf(":");
  const kind = at < 0 ? value : value.slice(0, at);
  if (!isKind(kind)) return undefined;
  if (at < 0) return { kind, id: null };
  // The last 7 days always end today, so there is nothing to pin.
  if (kind === "7d") return undefined;
  const id = value.slice(at + 1);
  return periodFromId(kind, id, now) ? { kind, id } : undefined;
}

/** A saved value that is no longer a period reads as the last 7 days. */
export const readPeriodPick = (now: Date = new Date()): PeriodPick =>
  parsePick(read(RANGE_KEY), now) ?? { kind: "7d", id: null };

export const writePeriodPick = (pick: PeriodPick) =>
  write(RANGE_KEY, serializePick(pick));

export function readMerge(): boolean {
  return read(MERGE_KEY) !== "false";
}

export const writeMerge = (merge: boolean) => write(MERGE_KEY, String(merge));
