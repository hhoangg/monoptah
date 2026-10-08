/**
 * The gate in front of the quit hotkey, the way Chrome does it: one press only
 * puts a notice on screen, and the app quits when the key is held down or
 * pressed a second time. A mistyped Cmd+Q is a lost session otherwise.
 *
 * Release is never part of this. macOS does not deliver a key-up for a letter
 * while Command is down, so "still held" is read from the key repeats the OS
 * sends instead — which is why a press carries whether it is a repeat.
 */

/** How long the key must stay down for the hold to count as a quit. */
export const QUIT_HOLD_MS = 1000;
/** How long after a press a second press still counts as confirming it. */
export const QUIT_ARM_MS = 2000;

export type QuitGateState = {
  /** When the key first went down, which the hold is measured from. */
  pressedAt: number;
  /** When the notice drops and the next press starts over. */
  expiresAt: number;
} | null;

export type QuitGateStep = { state: QuitGateState; quit: boolean };

export function isQuitHotkey(event: KeyboardEvent, isMac: boolean): boolean {
  if (event.key.toLowerCase() !== "q") return false;
  if (event.altKey || event.shiftKey) return false;
  return isMac
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
}

export function quitGatePress(
  state: QuitGateState,
  now: number,
  repeat: boolean,
): QuitGateStep {
  const armed = state && now <= state.expiresAt ? state : null;

  if (!armed) {
    // A repeat with nothing armed is the tail of a press this window never
    // saw — a key held across a window switch, say. Starting the gate from it
    // would arm with a hold that is already half spent.
    if (repeat) return { state: null, quit: false };
    return { state: { pressedAt: now, expiresAt: now + QUIT_ARM_MS }, quit: false };
  }

  if (repeat) {
    if (now - armed.pressedAt >= QUIT_HOLD_MS) return { state: null, quit: true };
    return {
      state: { pressedAt: armed.pressedAt, expiresAt: now + QUIT_ARM_MS },
      quit: false,
    };
  }

  return { state: null, quit: true };
}
