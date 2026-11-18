// Ported from mobile/src/features/sessions/TerminalSpinner.tsx (same frames and speed).
import { useSyncExternalStore } from "react";

const FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] as const;
const FRAME_MS = 80;

// One clock for every spinner on screen, so they step in one render rather than each on its own
// timer, and none runs while none is shown.
let frame = 0;
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  timer ??= setInterval(() => {
    frame = (frame + 1) % FRAMES.length;
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
const current = () => frame;

/** `prefers-reduced-motion: reduce` holds the spinner on its first frame instead of stepping. */
function subscribeReducedMotion(listener: () => void) {
  const query = window.matchMedia("(prefers-reduced-motion: reduce)");
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}
const reducedMotion = () =>
  typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const serverReducedMotion = () => false;

function Stepping({ className }: { className?: string }) {
  const shown = useSyncExternalStore(subscribe, current, current);
  return (
    <span aria-hidden="true" className={className}>
      {FRAMES[shown]}
    </span>
  );
}

/** Braille working spinner of design-system.md section 8. Decorative: never announced. */
export function TerminalSpinner({ className }: { className?: string }) {
  const reduced = useSyncExternalStore(
    typeof window !== "undefined" && typeof window.matchMedia === "function" ? subscribeReducedMotion : () => () => {},
    reducedMotion,
    serverReducedMotion,
  );
  if (reduced) {
    return (
      <span aria-hidden="true" className={className}>
        {FRAMES[0]}
      </span>
    );
  }
  return <Stepping className={className} />;
}

export const TERMINAL_SPINNER_FRAMES: readonly string[] = FRAMES;
