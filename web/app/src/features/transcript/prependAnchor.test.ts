import { describe, expect, it } from "vitest";
import { captureAnchor, restoreAnchor } from "./prependAnchor";

const rows = (starts: number[]) => starts.map((start, index) => ({ key: `r${index}`, start }));

describe("prependAnchor", () => {
  it("captures the first row that ends below the top edge", () => {
    const measured = rows([0, 100, 250, 400]);
    const sizes = [100, 150, 150, 100];
    expect(captureAnchor(measured, (i) => sizes[i]!, 120)).toEqual({ key: "r1", delta: -20 });
    expect(captureAnchor(measured, (i) => sizes[i]!, 0)).toEqual({ key: "r0", delta: 0 });
    expect(captureAnchor(measured, (i) => sizes[i]!, 9999)).toBeNull();
  });

  it("puts the anchored row back where it was after rows landed above it", () => {
    const before = rows([0, 100, 250]);
    const anchor = captureAnchor(before, () => 100, 130)!;
    // Two 80 px turns arrive above: every start moves by 160.
    const after = [{ key: "n1", start: 0 }, { key: "n2", start: 80 }, ...before.map((row) => ({ ...row, start: row.start + 160 }))];
    expect(restoreAnchor(after, anchor)).toBe(130 + 160);
  });

  it("never scrolls above the top and gives up when the row is gone", () => {
    expect(restoreAnchor(rows([0]), { key: "r0", delta: 50 })).toBe(0);
    expect(restoreAnchor(rows([0]), { key: "missing", delta: 0 })).toBeNull();
  });
});
