import { act, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { findScrollParent, useWindowedList } from "./useWindowedList";

function List({ count, onRange }: { count: number; onRange: (range: { start: number; end: number }) => void }) {
  const [ref, range] = useWindowedList(count, 100, 0);
  onRange(range);
  return <ul ref={ref} />;
}

describe("useWindowedList", () => {
  it("finds the nearest scrollable ancestor and falls back to the window", () => {
    const { container } = render(
      <div style={{ overflowY: "auto" }} data-testid="pane">
        <div>
          <ul />
        </div>
      </div>,
    );
    const list = container.querySelector("ul") as HTMLElement;
    expect(findScrollParent(list)).toBe(container.querySelector("[data-testid=pane]"));
    const plain = render(<ul />).container.querySelector("ul") as HTMLElement;
    expect(findScrollParent(plain)).toBeNull();
  });

  it("subscribes to the scroll container, not the window, and re-measures on its scroll", () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    try {
      const windowSpy = vi.spyOn(window, "addEventListener");
      const ranges: { start: number; end: number }[] = [];
      const { container } = render(
        <div style={{ overflowY: "auto" }} data-testid="pane">
          <List count={100} onRange={(range) => ranges.push(range)} />
        </div>,
      );
      const pane = container.querySelector("[data-testid=pane]") as HTMLElement;
      const list = container.querySelector("ul") as HTMLElement;
      expect(windowSpy.mock.calls.some((call) => (call[0] as string) === "scroll")).toBe(false);
      Object.defineProperty(pane, "clientHeight", { value: 300, configurable: true });
      pane.getBoundingClientRect = () => ({ top: 0, bottom: 300 }) as DOMRect;
      list.getBoundingClientRect = () => ({ top: -1000 }) as DOMRect;
      act(() => {
        pane.dispatchEvent(new Event("scroll"));
      });
      const last = ranges[ranges.length - 1]!;
      expect(last.start).toBe(10);
      expect(last.end).toBe(13);
      windowSpy.mockRestore();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
