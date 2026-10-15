import { DEFAULT_RANGE, parseRange } from "./range";
import type { RangeKey } from "./range";

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

export const readRange = (): RangeKey =>
  parseRange(read(RANGE_KEY)) ?? DEFAULT_RANGE;

export const writeRange = (range: RangeKey) => write(RANGE_KEY, range);

export function readMerge(): boolean {
  return read(MERGE_KEY) !== "false";
}

export const writeMerge = (merge: boolean) => write(MERGE_KEY, String(merge));
