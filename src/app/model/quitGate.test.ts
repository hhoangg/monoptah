import { describe, expect, it } from "vitest";
import {
  QUIT_ARM_MS,
  QUIT_HOLD_MS,
  isQuitHotkey,
  quitGatePress,
} from "./quitGate";

const key = (over: Partial<KeyboardEvent> = {}) =>
  ({
    key: "q",
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...over,
  }) as KeyboardEvent;

describe("isQuitHotkey", () => {
  it("takes Command+Q on a Mac and Control+Q elsewhere", () => {
    expect(isQuitHotkey(key({ metaKey: true }), true)).toBe(true);
    expect(isQuitHotkey(key({ ctrlKey: true }), false)).toBe(true);
  });

  it("ignores the other platform's modifier", () => {
    expect(isQuitHotkey(key({ ctrlKey: true }), true)).toBe(false);
    expect(isQuitHotkey(key({ metaKey: true }), false)).toBe(false);
  });

  it("ignores another key, and the hotkey with extra modifiers", () => {
    expect(isQuitHotkey(key({ metaKey: true, key: "w" }), true)).toBe(false);
    expect(isQuitHotkey(key({ metaKey: true, shiftKey: true }), true)).toBe(
      false,
    );
    expect(isQuitHotkey(key({ metaKey: true, altKey: true }), true)).toBe(false);
  });

  it("accepts the key however the keyboard cased it", () => {
    expect(isQuitHotkey(key({ metaKey: true, key: "Q" }), true)).toBe(true);
  });
});

describe("quitGatePress", () => {
  it("arms on the first press instead of quitting", () => {
    const step = quitGatePress(null, 1000, false);

    expect(step.quit).toBe(false);
    expect(step.state).toEqual({ pressedAt: 1000, expiresAt: 1000 + QUIT_ARM_MS });
  });

  it("quits on a second press while armed", () => {
    const armed = quitGatePress(null, 1000, false).state;

    const step = quitGatePress(armed, 1000 + QUIT_ARM_MS - 1, false);

    expect(step.quit).toBe(true);
    expect(step.state).toBeNull();
  });

  it("arms again, rather than quitting, once the window has passed", () => {
    const armed = quitGatePress(null, 1000, false).state;

    const step = quitGatePress(armed, 1000 + QUIT_ARM_MS + 1, false);

    expect(step.quit).toBe(false);
    expect(step.state).not.toBeNull();
  });

  it("quits once a repeat says the key is still down past the hold", () => {
    const armed = quitGatePress(null, 1000, false).state;

    const step = quitGatePress(armed, 1000 + QUIT_HOLD_MS, true);

    expect(step.quit).toBe(true);
    expect(step.state).toBeNull();
  });

  it("keeps the notice up, without quitting, on an early repeat", () => {
    const armed = quitGatePress(null, 1000, false).state;
    const early = 1000 + QUIT_HOLD_MS - 1;

    const step = quitGatePress(armed, early, true);

    expect(step.quit).toBe(false);
    // Holding the key must not let the notice time out underneath it.
    expect(step.state).toEqual({ pressedAt: 1000, expiresAt: early + QUIT_ARM_MS });
  });

  it("ignores a repeat that arrives with nothing armed", () => {
    const step = quitGatePress(null, 1000, true);

    expect(step).toEqual({ state: null, quit: false });
  });
});
