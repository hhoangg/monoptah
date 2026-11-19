// Test helper: jsdom has no layout, so the geometry the virtualized transcript reads is stubbed. Every row is
// ROW px tall, the scroller VIEW px, and `scrollTop` is a plain settable value that reports itself as a scroll
// event (a browser does too). Call `stubTranscriptLayout()` once in `beforeAll`.
import { vi } from "vitest";

export const ROW = 100;
export const VIEW = 400;

export const scrollTops = new WeakMap<Element, number>();

export function stubTranscriptLayout(): void {
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get(this: HTMLElement) {
      if (this.dataset.testid === "transcript-scroller") return VIEW;
      return this.dataset.index !== undefined ? ROW : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-scroller" ? VIEW : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-scroller" ? parseFloat((this.firstElementChild as HTMLElement).style.height) || 0 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get(this: HTMLElement) {
      return scrollTops.get(this) ?? 0;
    },
    set(this: HTMLElement, value: number) {
      scrollTops.set(this, value);
      setTimeout(() => this.dispatchEvent(new Event("scroll")), 0);
    },
  });
  Element.prototype.scrollTo = function scrollTo(this: Element, options?: ScrollToOptions | number) {
    scrollTops.set(this, typeof options === "object" ? (options.top ?? 0) : 0);
  } as typeof Element.prototype.scrollTo;
}
