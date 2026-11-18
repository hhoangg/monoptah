// Copied from src/features/source-control/model/unifiedDiff.test.ts at 5c00fe2a
// (fold and reveal cases only), plus wire-line and edge cases.
import { describe, expect, it } from "vitest";
import { expandFold, foldUnifiedLines, revealedFold, type FoldableLine } from "./diffFold";

type Line = FoldableLine & { runs?: [string, number][] };

function rangeContext(from: number, to: number): Line[] {
  const lines: Line[] = [];
  for (let number = from; number <= to; number += 1) {
    lines.push({ kind: "context", text: `c${number}`, oldNumber: number, newNumber: number });
  }
  return lines;
}

const change = (): Line[] => [
  { kind: "del", text: "old", oldNumber: 9, newNumber: null },
  { kind: "add", text: "new", oldNumber: null, newNumber: 9 },
];

describe("foldUnifiedLines", () => {
  it("splits long context around a change into folds", () => {
    const lines = [...rangeContext(1, 8), ...change(), ...rangeContext(10, 16)];
    const blocks = foldUnifiedLines(lines, 2);
    expect(blocks.map((block) => block.kind)).toEqual(["fold", "hunk", "fold"]);
    const hunk = blocks[1]!;
    expect(hunk.kind).toBe("hunk");
    if (hunk.kind !== "hunk") return;
    expect(hunk.lines.map((line) => line.text)).toEqual(["c7", "c8", "old", "new", "c10", "c11"]);
    expect(blocks[0]!.lines).toHaveLength(6);
    expect(blocks[2]!.lines).toHaveLength(5);
  });

  it("keeps every line visible when context is unlimited", () => {
    const lines = [...rangeContext(1, 8), ...change(), ...rangeContext(10, 16)];
    const blocks = foldUnifiedLines(lines, Number.POSITIVE_INFINITY);
    expect(blocks.map((block) => block.kind)).toEqual(["hunk"]);
    expect(blocks[0]?.kind === "hunk" && blocks[0].lines).toEqual(lines);
  });

  it("keeps hunk headers visible so they split folds", () => {
    const blocks = foldUnifiedLines(
      [
        { kind: "hunk", text: "@@ -1,3 +1,3 @@", oldNumber: null, newNumber: null },
        { kind: "context", text: "keep", oldNumber: 1, newNumber: 1 },
        { kind: "add", text: "plus", oldNumber: null, newNumber: 2 },
      ] satisfies Line[],
      1,
    );
    expect(blocks[0]?.kind).toBe("hunk");
  });

  it("returns no blocks for no lines and gives each fold a distinct id", () => {
    expect(foldUnifiedLines([])).toEqual([]);
    const lines = [...rangeContext(1, 10), ...change(), ...rangeContext(10, 20), ...change(), ...rangeContext(30, 40)];
    const ids = foldUnifiedLines(lines, 1).flatMap((b) => (b.kind === "fold" ? [b.id] : []));
    expect(ids).toEqual(["fold-0", "fold-1", "fold-2"]);
  });

  it("preserves wire fields such as runs on the lines", () => {
    const lines: Line[] = [{ kind: "add", text: "x", oldNumber: null, newNumber: 1, runs: [["x", 1]] }];
    const block = foldUnifiedLines(lines)[0]!;
    expect(block.lines[0]!.runs).toEqual([["x", 1]]);
  });
});

describe("fold expansion", () => {
  it("reveals from the start when expanding down", () => {
    expect(revealedFold(10, expandFold(undefined, 10, "down", 3))).toEqual({ head: 3, tail: 0, hidden: 7 });
  });

  it("reveals from the end when expanding up", () => {
    const next = expandFold({ start: 2, end: 0 }, 10, "up", 3);
    expect(revealedFold(10, next)).toEqual({ head: 2, tail: 3, hidden: 5 });
  });

  it("expands the whole fold at once", () => {
    expect(revealedFold(12, expandFold(undefined, 12, "all"))).toEqual({ head: 12, tail: 0, hidden: 0 });
  });

  it("clamps a reveal larger than the fold", () => {
    expect(revealedFold(5, { start: 4, end: 9 })).toEqual({ head: 4, tail: 1, hidden: 0 });
    expect(revealedFold(5, expandFold({ start: 4, end: 0 }, 5, "down", 20))).toEqual({ head: 5, tail: 0, hidden: 0 });
  });
});
