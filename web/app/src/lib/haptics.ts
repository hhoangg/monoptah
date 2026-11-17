// Haptics abstraction of design-system.md section 10. Vibration exists on Android Chrome only;
// iOS and desktop browsers get a no-op, and every haptic point also has a visual press state.
export type Haptic = "light" | "medium" | "selection" | "success" | "warning" | "error";

const PATTERNS: Record<Haptic, number | number[]> = {
  light: 8,
  medium: 16,
  selection: 4,
  success: [10, 40, 10],
  warning: [20, 60, 20],
  error: [30, 50, 30, 50, 30],
};

export const HAPTICS_KEY = "monoptah.haptics";

function hapticsEnabled(): boolean {
  try {
    return window.localStorage.getItem(HAPTICS_KEY) !== "off"; // user setting, default on
  } catch {
    return true;
  }
}

/** Call from event handlers only: vibration needs a user gesture. */
export function haptic(kind: Haptic): void {
  if (typeof navigator === "undefined" || typeof navigator.vibrate !== "function") return;
  if (typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (!hapticsEnabled()) return;
  navigator.vibrate(PATTERNS[kind]);
}
