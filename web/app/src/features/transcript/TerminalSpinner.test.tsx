import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TERMINAL_SPINNER_FRAMES, TerminalSpinner } from "./TerminalSpinner";

function mockReduced(matches: boolean) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn().mockImplementation(() => ({
      matches,
      addEventListener: () => {},
      removeEventListener: () => {},
    })),
  );
}

describe("TerminalSpinner", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("steps through the braille frames every 80 ms and hides from assistive tech", () => {
    mockReduced(false);
    const { container, unmount } = render(<TerminalSpinner />);
    const el = container.firstElementChild as HTMLElement;
    expect(el.getAttribute("aria-hidden")).toBe("true");
    const start = TERMINAL_SPINNER_FRAMES.indexOf(el.textContent ?? "");
    expect(start).toBeGreaterThanOrEqual(0);
    act(() => {
      vi.advanceTimersByTime(80);
    });
    expect(el.textContent).toBe(TERMINAL_SPINNER_FRAMES[(start + 1) % TERMINAL_SPINNER_FRAMES.length]);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("holds the first frame under prefers-reduced-motion", () => {
    mockReduced(true);
    const { container } = render(<TerminalSpinner />);
    act(() => {
      vi.advanceTimersByTime(400);
    });
    expect(container.textContent).toBe(TERMINAL_SPINNER_FRAMES[0]);
    expect(vi.getTimerCount()).toBe(0);
  });
});
