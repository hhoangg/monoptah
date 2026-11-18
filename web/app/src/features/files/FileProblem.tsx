// F-62: the illustration and message of a file that cannot be shown (hidden, missing or failed).
// A page with a folded corner drawn as React SVG elements (no SVG string, no HTML sink).
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/** The '#' cells of a 16 x 13 pixel page with a question mark (the mobile app's FileMissing). */
const PAGE = [
  "...#######......",
  "...#.....##.....",
  "...#.....#.#....",
  "...#.....####...",
  "...#........#...",
  "...#...##...#...",
  "...#..#..#..#...",
  "...#.....#..#...",
  "...#....#...#...",
  "...#........#...",
  "...#....#...#...",
  "...#........#...",
  "...##########...",
];

const CELLS = PAGE.flatMap((row, y) => [...row].flatMap((cell, x) => (cell === "#" ? [{ x, y }] : [])));

export interface FileProblemProps {
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
  className?: string;
}

export function FileProblem({ title, children, actions, className }: FileProblemProps) {
  return (
    <div role="status" data-testid="file-problem" className={cn("flex flex-col items-center justify-center gap-3 px-6 py-10 text-center", className)}>
      <svg aria-hidden="true" width={96} height={78} viewBox="0 0 16 13" className="text-content opacity-25" shapeRendering="crispEdges">
        {CELLS.map(({ x, y }) => (
          <rect key={`${x}-${y}`} x={x} y={y} width={1} height={1} fill="currentColor" />
        ))}
      </svg>
      <p className="text-title font-semibold">{title}</p>
      {children ? <p className="max-w-prose text-small text-muted-text">{children}</p> : null}
      {actions}
    </div>
  );
}
