// F-91: a project's 8x8 pixel mascot as an SVG (ported from mobile/src/features/mascots/ProjectMascot.tsx).
// It is drawn in the project's colour; while a turn is in flight it swaps between its rest and talk frames
// every 300 ms, unless the user prefers reduced motion. One clock serves every mascot at work.
import { useSyncExternalStore } from "react";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { MASCOT_GRID, projectMascot } from "./projectMascots";

/** Matches the desktop's 600 ms rest and talk cycle (300 ms a frame). */
export const FRAME_MS = 300;
export const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

let talking = false;
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  timer ??= setInterval(() => {
    talking = !talking;
    listeners.forEach((notify) => notify());
  }, FRAME_MS);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}
const still = () => () => undefined;

/** Flips between the rest and talk frames while `active`; stays on the rest frame when `paused`. */
function useFrameToggle(active: boolean, paused: boolean): boolean {
  const running = active && !paused;
  return useSyncExternalStore(
    running ? subscribe : still,
    () => (running ? talking : false),
    () => false,
  );
}

export interface ProjectMascotProps {
  /** The hash input: the project's path (as mobile does) or id. */
  project: string;
  /** A `#rrggbb` colour, already checked by the caller. */
  color: string;
  /** The explicit pick from the desktop; an unknown name falls back to the hashed one. */
  name?: string | null;
  size?: number;
  /** Cycles the two frames while a turn is in flight. */
  active?: boolean;
  className?: string;
}

export function ProjectMascot({ project, color, name, size = 18, active = false, className }: ProjectMascotProps) {
  const mascot = projectMascot(project, name);
  const reduced = useMediaQuery(REDUCED_MOTION_QUERY);
  const frame = useFrameToggle(active, reduced);
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      width={size}
      height={size}
      viewBox={`0 0 ${MASCOT_GRID} ${MASCOT_GRID}`}
      shapeRendering="crispEdges"
      className={className}
      data-mascot={mascot.name}
      data-frame={frame ? "talk" : "rest"}
    >
      <path d={frame ? mascot.talkPath : mascot.restPath} fill={color} />
    </svg>
  );
}
