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

  it("listens in the capture phase and re-measures on a pane scroll, also after the scroll parent changes", () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    try {
      const documentSpy = vi.spyOn(document, "addEventListener");
      const ranges: { start: number; end: number }[] = [];
      const { container } = render(
        <div data-testid="outer">
          <div style={{ overflowY: "auto" }} data-testid="pane">
            <List count={100} onRange={(range) => ranges.push(range)} />
          </div>
        </div>,
      );
      const pane = container.querySelector("[data-testid=pane]") as HTMLElement;
      const list = container.querySelector("ul") as HTMLElement;
      expect(documentSpy.mock.calls.some((call) => call[0] === "scroll" && (call[2] as AddEventListenerOptions).capture === true)).toBe(true);
      Object.defineProperty(pane, "clientHeight", { value: 300, configurable: true });
      pane.getBoundingClientRect = () => ({ top: 0, bottom: 300 }) as DOMRect;
      list.getBoundingClientRect = () => ({ top: -1000 }) as DOMRect;
      act(() => {
        pane.dispatchEvent(new Event("scroll"));
      });
      const last = ranges[ranges.length - 1]!;
      expect(last.start).toBe(10);
      expect(last.end).toBe(13);
      // The scroll parent changes without a remount (split <-> stacked): the same listener follows it.
      const outer = container.querySelector("[data-testid=outer]") as HTMLElement;
      outer.style.overflowY = "auto";
      Object.defineProperty(outer, "clientHeight", { value: 200, configurable: true });
      outer.getBoundingClientRect = () => ({ top: 0, bottom: 200 }) as DOMRect;
      pane.style.overflowY = "visible";
      list.getBoundingClientRect = () => ({ top: -2000 }) as DOMRect;
      act(() => {
        outer.dispatchEvent(new Event("scroll"));
      });
      const moved = ranges[ranges.length - 1]!;
      expect(moved.start).toBe(20);
      expect(moved.end).toBe(22);
      documentSpy.mockRestore();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
