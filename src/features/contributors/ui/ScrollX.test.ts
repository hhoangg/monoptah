// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ScrollX } from "./ScrollX";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const mount = () =>
  act(async () => {
    root.render(
      createElement(ScrollX, { label: "Ranking table" }, createElement("i")),
    );
  });
const scroller = () =>
  container.querySelector<HTMLElement>('[role="region"]') as HTMLElement;

function measure(el: HTMLElement, sizes: Record<string, number>) {
  for (const [key, value] of Object.entries(sizes)) {
    Object.defineProperty(el, key, { value, configurable: true });
  }
}
const scrollTo = (el: HTMLElement, left: number) =>
  act(async () => {
    measure(el, { scrollLeft: left });
    el.dispatchEvent(new Event("scroll"));
  });
const mask = (el: HTMLElement) =>
  el.style.getPropertyValue("mask-image") ||
  el.style.getPropertyValue("-webkit-mask-image");

describe("ScrollX", () => {
  it("is a focusable labelled region so the keyboard can scroll it", async () => {
    await mount();
    expect(scroller().getAttribute("aria-label")).toBe("Ranking table");
    expect(scroller().tabIndex).toBe(0);
    expect(scroller().className).toContain("overflow-x-auto");
  });

  it("shows no fade when everything fits", async () => {
    await mount();
    measure(scroller(), { clientWidth: 400, scrollWidth: 400 });
    await scrollTo(scroller(), 0);
    expect(mask(scroller())).toBe("");
  });

  it("fades the right edge while more content waits there", async () => {
    await mount();
    measure(scroller(), { clientWidth: 100, scrollWidth: 300 });
    await scrollTo(scroller(), 0);
    expect(mask(scroller())).toContain("linear-gradient");
    expect(scroller().dataset.fade).toBe("end");
  });

  it("fades both edges mid-scroll and only the left at the end", async () => {
    await mount();
    measure(scroller(), { clientWidth: 100, scrollWidth: 300 });
    await scrollTo(scroller(), 80);
    expect(scroller().dataset.fade).toBe("both");
    await scrollTo(scroller(), 200);
    expect(scroller().dataset.fade).toBe("start");
  });
});
