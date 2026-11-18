// F-73, F-74: the visual tree renderer draws every node kind as React elements and inline SVG, with no HTML sink.
import { render, screen } from "@testing-library/react";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ContributionGraph } from "./charts";
import { inked } from "./dither";
import { VisualTree } from "./VisualTree";
import type { VisualNode } from "./types";

const series = [{ label: "A", values: [1, 3, 2, 5] }, { label: "B", values: [2, 1, 4, 3], color: "series3" as const }];
const labels = ["Mon", "Tue", "Wed", "Thu"];

const KINDS: Record<string, VisualNode> = {
  bar: { kind: "chart", chart_type: "bar", series, labels, height: 200 },
  line: { kind: "chart", chart_type: "line", series, labels, height: 200 },
  area: { kind: "chart", chart_type: "area", series, labels, height: 200 },
  gauge: { kind: "gauge", label: "CPU", value: 40, min: 0, max: 100, unit: "%", color: "accent" },
  donut: { kind: "donut", slices: [{ label: "x", value: 3 }, { label: "y", value: 1, color: "series3" }], label: "Total", value: null },
  heatmap: { kind: "heatmap", values: [[0, 1, 2], [3, 4, 0]], rows: ["a", "b"], columns: ["1", "2", "3"], color: "accent" },
  progress: { kind: "progress", label: "Build", value: 30, max: 60, detail: "half", color: "positive" },
  canvas: {
    kind: "canvas",
    width: 100,
    height: 50,
    shapes: [
      { kind: "rect", x: 1, y: 1, width: 10, height: 10, color: "accent" },
      { kind: "circle", x: 20, y: 20, radius: 5, color: "positive" },
      { kind: "line", x1: 0, y1: 0, x2: 9, y2: 9, width: 1, color: "ink" },
      { kind: "polyline", points: [[0, 0], [5, 5], [9, 2]], width: 1, color: "warning" },
      { kind: "label", x: 4, y: 4, text: "hi", color: "ink" },
    ],
  },
};

describe("visual tree (F-73)", () => {
  it.each(Object.entries(KINDS))("draws the %s node as inline SVG", (_name, node) => {
    const { container } = render(<VisualTree node={node} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("viewBox")).toMatch(/^0 0 \d+ \d+$/);
    // The halftone dither is painted as filled paths, one per colour.
    if (["bar", "area", "gauge", "donut", "heatmap", "progress"].includes(_name)) expect(container.querySelectorAll("svg path").length).toBeGreaterThan(0);
  });

  it("draws a stat with its trend, a table and the layout nodes", () => {
    const tree: VisualNode = {
      kind: "column",
      gap: 8,
      children: [
        { kind: "text", text: "Weekly report", size: "title", color: "ink" },
        { kind: "row", gap: 8, children: [{ kind: "stat", label: "Revenue", value: "$12k", detail: "+12.4% vs last month", color: "accent", trend: [1, 2, 4, 3] }] },
        { kind: "grid", columns: 2, gap: 8, children: [{ kind: "table", columns: ["Name", "Qty"], rows: [["Widget", "4"]] }] },
        { kind: "card", gap: 4, children: [{ kind: "progress", label: "Goal", value: 1, max: 2, color: "accent" }] },
      ],
    };
    const { container } = render(<VisualTree node={tree} />);
    expect(screen.getByText("Weekly report")).toBeInTheDocument();
    expect(screen.getByText("$12k")).toBeInTheDocument();
    expect(screen.getByText(/\+12\.4%/)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Qty" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "Widget" })).toBeInTheDocument();
    // The stat's trend and the progress bar are SVG.
    expect(container.querySelectorAll("svg").length).toBeGreaterThanOrEqual(2);
  });

  it("shows interactive nodes as static state, never as live controls", () => {
    const tree: VisualNode = {
      kind: "column",
      gap: 4,
      children: [
        { kind: "button", label: "Run", handler: "h", disabled: false },
        { kind: "toggle", label: "Auto", value: true, handler: "h" },
        { kind: "input", label: "Name", value: "x", handler: "h" },
        { kind: "select", label: "Mode", options: ["a", "b"], selected: 1, handler: "h" },
        { kind: "slider", label: "Level", value: 5, min: 0, max: 10, step: 1, handler: "h" },
        { kind: "tabs", labels: ["One", "Two"], selected: 0, handler: "h" },
      ],
    };
    const { container } = render(<VisualTree node={tree} />);
    expect(screen.getByText("Run")).toBeInTheDocument();
    expect(container.querySelector("button, input, select, textarea")).toBeNull();
  });

  it("renders hostile text as text, and the sources contain no HTML sink", () => {
    const hostile = '<img src=x onerror="alert(1)"><script>alert(1)</script>';
    const { container } = render(<VisualTree node={{ kind: "text", text: hostile, size: "body", color: "ink" }} />);
    expect(container.querySelector("img, script")).toBeNull();
    expect(container.textContent).toContain(hostile);
    for (const file of readdirSync(__dirname).filter((name) => /\.tsx?$/.test(name) && !name.endsWith(".test.tsx"))) {
      expect(readFileSync(join(__dirname, file), "utf8"), file).not.toMatch(/dangerouslySetInnerHTML|innerHTML|insertAdjacentHTML|DOMParser/);
    }
  });

  it("draws nothing for an empty chart and a 3D scene as a notice", () => {
    const empty = render(<VisualTree node={{ kind: "chart", chart_type: "bar", series: [], labels: [], height: 100 }} />);
    expect(empty.container.querySelector("svg")).toBeNull();
    empty.unmount();
    render(<VisualTree node={{ kind: "scene", width: 4, height: 3 }} />);
    expect(screen.getByText(/3D scene on your desktop/)).toBeInTheDocument();
  });
});

/** How many of the 16 cells of one Bayer tile are inked at `tone`. */
const dither = (tone: number) => Array.from({ length: 16 }, (_, i) => inked(i % 4, Math.floor(i / 4), tone)).filter(Boolean).length;

describe("dither", () => {
  it("inks a share of cells that matches the tone", () => {
    expect(dither(0.25)).toBe(4);
    expect(dither(0)).toBe(0);
    expect(dither(1)).toBe(16);
  });
});

describe("contribution graph (F-74)", () => {
  it("draws 26 weeks as a grid with weekday and month labels", () => {
    const levels = Array.from({ length: 26 * 7 }, (_, i) => i % 5);
    const { container } = render(<ContributionGraph levels={levels} today={new Date(2026, 10, 18)} />);
    const svg = container.querySelector("svg") as SVGSVGElement;
    // 26 weeks of 5 cells of 3 units, after the 28 unit gutter.
    expect(svg.getAttribute("viewBox")).toBe(`0 0 ${28 + 26 * 15} ${18 + 7 * 15}`);
    expect(["Mon", "Wed", "Fri"].every((day) => screen.getByText(day))).toBe(true);
    expect(container.querySelectorAll("text").length).toBeGreaterThan(5);
    expect(container.querySelectorAll("path").length).toBeGreaterThan(0);
  });
});
