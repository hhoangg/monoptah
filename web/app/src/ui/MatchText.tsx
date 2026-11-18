// Copied from src/shared/ui/MatchText.tsx at c579af2e
import type { ReactNode } from "react";

/**
 * Renders `text` with the characters at `positions` (from `fuzzyMatch` in
 * `@/lib/fuzzy`) accented. `active=false` or no positions renders plain text.
 * Pure React text and spans, no HTML injection.
 */
export type MatchTextProps = {
  text: string;
  positions: readonly number[];
  active: boolean;
};

export function MatchText({ text, positions, active }: MatchTextProps): ReactNode {
  if (!active || positions.length === 0) return text;
  const marked = new Set(positions);
  const runs: { text: string; match: boolean }[] = [];
  for (let i = 0; i < text.length; i++) {
    const match = marked.has(i);
    const last = runs[runs.length - 1];
    if (last && last.match === match) last.text += text[i] ?? "";
    else runs.push({ text: text[i] ?? "", match });
  }
  return runs.map((run, index) =>
    run.match ? (
      <span key={index} className="text-accent" data-match="">
        {run.text}
      </span>
    ) : (
      <span key={index}>{run.text}</span>
    ),
  );
}
