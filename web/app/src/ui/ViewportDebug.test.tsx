import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ViewportDebug from "./ViewportDebug";
import { ViewportDebugGate } from "./ViewportDebugGate";
import { setViewportDebug, viewportDebugCommand, viewportDebugEnabled } from "./viewportDebugFlag";

describe("viewportDebugEnabled", () => {
  beforeEach(() => window.sessionStorage.clear());
  afterEach(() => window.sessionStorage.clear());
  it("is off by default", () => {
    expect(viewportDebugEnabled("")).toBe(false);
    const { container } = render(<ViewportDebugGate />);
    expect(container).toBeEmptyDOMElement();
  });
  it("turns on with ?debug=viewport and persists", () => {
    expect(viewportDebugEnabled("?debug=viewport")).toBe(true);
    expect(window.sessionStorage.getItem("monoptah.debug.viewport")).toBe("1");
    expect(viewportDebugEnabled("")).toBe(true);
  });
  it("?debug=off clears it", () => {
    viewportDebugEnabled("?debug=viewport");
    expect(viewportDebugEnabled("?debug=off")).toBe(false);
    expect(window.sessionStorage.getItem("monoptah.debug.viewport")).toBeNull();
  });
  it("the overlay shows the labels", () => {
    render(<ViewportDebug />);
    expect(screen.getByTestId("viewport-debug").textContent).toContain("inner");
    expect(screen.getByTestId("viewport-debug").textContent).toContain("kbd typing");
  });
  it("the hidden palette command matches only the exact phrases", () => {
    expect(viewportDebugCommand(" Debug Viewport ")).toBe(true);
    expect(viewportDebugCommand("debug off")).toBe(false);
    expect(viewportDebugCommand("debug")).toBeNull();
    expect(viewportDebugCommand("debug viewports")).toBeNull();
    setViewportDebug(true);
    expect(viewportDebugEnabled("")).toBe(true);
    setViewportDebug(false);
    expect(viewportDebugEnabled("")).toBe(false);
  });
});
