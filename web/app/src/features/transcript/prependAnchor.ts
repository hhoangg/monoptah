// Keeps the reader's place when turns land above the viewport (F-32, N-16). Before the page is asked for,
// `captureAnchor` notes which row is at the top of the viewport and how far below the scroller's top it
// sits. Once the rows changed, `restoreAnchor` scrolls so that row sits at the same distance again.
// Browsers with native scroll anchoring (Chrome, Firefox) do not help a virtualized list, whose rows are
// absolutely positioned; iOS Safari has none at all, so the offset is always set by hand.
export interface ScrollAnchor {
  key: string;
  /** Distance from the scroller's top edge to the row's top edge when captured (can be negative). */
  delta: number;
}

export interface MeasuredRow {
  key: string;
  start: number;
}

/** The first row that ends below the top edge, with its offset in the viewport. */
export function captureAnchor(rows: readonly MeasuredRow[], sizes: (index: number) => number, scrollTop: number): ScrollAnchor | null {
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index]!;
    if (row.start + sizes(index) > scrollTop) return { key: row.key, delta: row.start - scrollTop };
  }
  return null;
}

/** The `scrollTop` that puts the anchored row back where it was, or null when it is gone. */
export function restoreAnchor(rows: readonly MeasuredRow[], anchor: ScrollAnchor): number | null {
  const row = rows.find((candidate) => candidate.key === anchor.key);
  return row ? Math.max(0, row.start - anchor.delta) : null;
}
