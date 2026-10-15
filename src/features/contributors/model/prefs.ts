import { DEFAULT_RANGE, parseRange, serializeRange } from "./range";
import type { RangeValue } from "./range";

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

/** A saved value that is no longer a range (the removed 90d, 1y, all) reads as the default. */
export const readRange = (now: Date = new Date()): RangeValue =>
  parseRange(read(RANGE_KEY), now) ?? DEFAULT_RANGE;

export const writeRange = (range: RangeValue) =>
  write(RANGE_KEY, serializeRange(range));

export function readMerge(): boolean {
  return read(MERGE_KEY) !== "false";
}

export const writeMerge = (merge: boolean) => write(MERGE_KEY, String(merge));
