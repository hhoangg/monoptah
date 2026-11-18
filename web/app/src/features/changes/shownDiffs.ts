// The last diffs the user opened, kept in memory so a diff screen still shows its file after a poll no
// longer lists it (for example right after a commit). Copied in spirit from mobile/.../shownDiffs.ts.
import type { ChangedFile } from "./gitModel";

export const MAX_KEPT_DIFFS = 4;

const kept = new Map<string, ChangedFile>();
const keyOf = (projectId: string, relative: string) => `${projectId}\0${relative}`;

/** Remembers a diff; the oldest beyond `MAX_KEPT_DIFFS` is dropped. */
export function keepDiff(projectId: string, file: ChangedFile): void {
  const key = keyOf(projectId, file.relative);
  kept.delete(key);
  kept.set(key, file);
  for (const old of kept.keys()) {
    if (kept.size <= MAX_KEPT_DIFFS) break;
    kept.delete(old);
  }
}

export function keptDiff(projectId: string, relative: string): ChangedFile | undefined {
  return kept.get(keyOf(projectId, relative));
}

export function keptDiffCount(): number {
  return kept.size;
}

export function clearKeptDiffs(): void {
  kept.clear();
}
