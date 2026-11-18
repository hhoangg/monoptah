// Copied from src/features/source-control/model/unifiedDiff.ts at 5c00fe2a
// (fold and reveal logic only: the CodeMirror diff builder, `pos` and the
// document helpers are dropped; lines are the wire lines of `project.changes`).
/**
 * Context folding for a unified diff.
 *
 * API (pure, generic over the wire line, so extra fields such as `runs` survive):
 * - `foldUnifiedLines(lines, context = 3)` -> `UnifiedBlock[]`: `hunk` blocks hold
 *   changed lines plus `context` lines around them and `hunk` header lines; long
 *   unchanged `context` stretches become `fold` blocks with a stable `id`.
 * - `revealedFold(total, reveal)` -> `{head, tail, hidden}`: how many lines of a
 *   fold show at its top (`head`) and bottom (`tail`), and how many stay hidden.
 * - `expandFold(reveal, total, "up" | "down" | "all", step = 20)` -> next `FoldReveal`.
 *   Keep one `FoldReveal` per fold `id` in component state.
 */

export const UNIFIED_CONTEXT_DEFAULT = 3;
export const UNIFIED_FOLD_STEP = 20;

export type UnifiedLineKind = "add" | "del" | "context" | "hunk";

/** Structural subset of a `project.changes` diff line (`@monoptah/protocol`). */
export type FoldableLine = {
  kind: UnifiedLineKind;
  text: string;
  oldNumber: number | null;
  newNumber: number | null;
};

export type UnifiedBlock<L extends FoldableLine = FoldableLine> =
  | { kind: "hunk"; lines: L[] }
  | { kind: "fold"; id: string; lines: L[] };

export type FoldReveal = { start: number; end: number };

export function foldUnifiedLines<L extends FoldableLine>(
  lines: readonly L[],
  context = UNIFIED_CONTEXT_DEFAULT,
): UnifiedBlock<L>[] {
  if (lines.length === 0) return [];
  const visible = visibleContext(lines, context);
  const blocks: UnifiedBlock<L>[] = [];
  let index = 0;
  let foldId = 0;
  while (index < lines.length) {
    const line = lines[index]!;
    if (line.kind === "context" && !visible[index]) {
      const start = index;
      while (index < lines.length && lines[index]!.kind === "context" && !visible[index]) {
        index += 1;
      }
      blocks.push({ kind: "fold", id: `fold-${foldId}`, lines: lines.slice(start, index) });
      foldId += 1;
      continue;
    }
    const start = index;
    while (index < lines.length) {
      const next = lines[index]!;
      if (next.kind === "context" && !visible[index]) break;
      index += 1;
    }
    blocks.push({ kind: "hunk", lines: lines.slice(start, index) });
  }
  return blocks;
}

export function revealedFold(
  total: number,
  reveal: FoldReveal | undefined,
): { head: number; tail: number; hidden: number } {
  const start = Math.max(0, Math.min(reveal?.start ?? 0, total));
  const remaining = total - start;
  const end = Math.max(0, Math.min(reveal?.end ?? 0, remaining));
  return {
    head: start,
    tail: end,
    hidden: total - start - end,
  };
}

export function expandFold(
  reveal: FoldReveal | undefined,
  total: number,
  direction: "up" | "down" | "all",
  step = UNIFIED_FOLD_STEP,
): FoldReveal {
  if (direction === "all" || total <= 0) return { start: total, end: 0 };
  const current = reveal ?? { start: 0, end: 0 };
  if (direction === "down") {
    return { start: current.start + step, end: current.end };
  }
  return { start: current.start, end: current.end + step };
}

function visibleContext(lines: readonly FoldableLine[], context: number): boolean[] {
  const visible = lines.map((line) => line.kind !== "context");
  if (context <= 0) return visible;
  for (let index = 0; index < lines.length; index += 1) {
    const kind = lines[index]!.kind;
    if (kind === "context" || kind === "hunk") continue;
    const from = Math.max(0, index - context);
    const to = Math.min(lines.length - 1, index + context);
    for (let inner = from; inner <= to; inner += 1) {
      if (lines[inner]!.kind === "context") visible[inner] = true;
    }
  }
  return visible;
}
