// Reads a visual tree out of a document or profile payload (F-73). The tree is data from the desktop, which bounds it
// when it is made (crates/monocode-core/src/visual/model.rs: 512 nodes, 16 levels, chart height 120 to 900, 512
// values per series, 512 shapes), but this is a different machine than the one that checked it: every field the
// renderer reads is checked here, the same bounds are applied, and a tree that does not fit is not drawn (null)
// rather than crashing the page. A node kind this app does not know is skipped, as the renderer always did.
import type { Series, Shape, Slice, TextSize, ThemeColor, VisualNode } from "./types";

export const MAX_NODES = 512;
export const MAX_DEPTH = 16;
export const MAX_SERIES = 8;
export const MAX_VALUES = 512;
export const CHART_HEIGHT = { min: 120, max: 900, fallback: 220 } as const;

const KNOWN = new Set([
  "column", "row", "card", "grid", "text", "stat", "table", "chart", "gauge", "donut", "heatmap", "progress",
  "button", "toggle", "input", "select", "slider", "tabs", "canvas", "scene",
]);
const SIZES: readonly TextSize[] = ["caption", "small", "body", "subtitle", "title", "hero"];

class Malformed extends Error {}
const bad = (): never => {
  throw new Malformed();
};

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const num = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? value : bad());
const str = (value: unknown): string => (typeof value === "string" ? value : bad());
const optStr = (value: unknown): string | null | undefined => (value === undefined || value === null ? value : str(value));
const arr = (value: unknown, max = MAX_VALUES): unknown[] => (Array.isArray(value) && value.length <= max ? value : bad());
const nums = (value: unknown) => arr(value).map(num);
const strs = (value: unknown) => arr(value).map(str);
const color = (value: unknown): ThemeColor => (typeof value === "string" ? (value as ThemeColor) : "accent");
const gap = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? Math.min(Math.max(value, 0), 64) : 8);
const dimension = (value: unknown): number => Math.min(Math.max(num(value), 1), 4096);

function shape(value: unknown): Shape {
  if (!isRecord(value)) return bad();
  const c = color(value.color);
  switch (value.kind) {
    case "rect":
      return { kind: "rect", x: num(value.x), y: num(value.y), width: num(value.width), height: num(value.height), color: c };
    case "circle":
      return { kind: "circle", x: num(value.x), y: num(value.y), radius: num(value.radius), color: c };
    case "line":
      return { kind: "line", x1: num(value.x1), y1: num(value.y1), x2: num(value.x2), y2: num(value.y2), width: num(value.width), color: c };
    case "polyline":
      return {
        kind: "polyline",
        points: arr(value.points).map((point) => {
          const pair = arr(point, 2);
          return [num(pair[0]), num(pair[1])] as [number, number];
        }),
        width: num(value.width),
        color: c,
      };
    case "label":
      return { kind: "label", x: num(value.x), y: num(value.y), text: str(value.text), color: c };
    default:
      return bad();
  }
}

/** One node; null for a kind this app does not know. Throws `Malformed` for a known kind with the wrong shape. */
function node(value: unknown, budget: { nodes: number }, depth: number): VisualNode | null {
  if (!isRecord(value) || typeof value.kind !== "string") return bad();
  if (!KNOWN.has(value.kind)) return null;
  budget.nodes += 1;
  if (budget.nodes > MAX_NODES || depth > MAX_DEPTH) return bad();
  const children = () =>
    arr(value.children, MAX_NODES)
      .map((child) => node(child, budget, depth + 1))
      .filter((child): child is VisualNode => child !== null);
  switch (value.kind) {
    case "column":
    case "row":
    case "card":
      return { kind: value.kind, children: children(), gap: gap(value.gap) };
    case "grid":
      return { kind: "grid", children: children(), columns: Math.min(Math.max(Math.round(num(value.columns)), 1), 12), gap: gap(value.gap) };
    case "text":
      return { kind: "text", text: str(value.text), size: SIZES.includes(value.size as TextSize) ? (value.size as TextSize) : "body", color: color(value.color) };
    case "stat":
      return {
        kind: "stat",
        label: str(value.label),
        value: str(value.value),
        detail: optStr(value.detail) ?? null,
        color: color(value.color),
        delta: optStr(value.delta) ?? null,
        ...(value.trend === undefined || value.trend === null ? {} : { trend: nums(value.trend) }),
        lower_is_better: value.lower_is_better === true,
      };
    case "table":
      return { kind: "table", columns: strs(value.columns), rows: arr(value.rows).map(strs) };
    case "chart": {
      const type = value.chart_type;
      if (type !== "bar" && type !== "line" && type !== "area") return bad();
      const series: Series[] = arr(value.series, MAX_SERIES).map((entry) => {
        if (!isRecord(entry)) return bad();
        return { label: str(entry.label), values: nums(entry.values), ...(typeof entry.color === "string" ? { color: entry.color as ThemeColor } : {}) };
      });
      const height = typeof value.height === "number" && Number.isFinite(value.height) ? value.height : CHART_HEIGHT.fallback;
      return { kind: "chart", chart_type: type, series, labels: strs(value.labels ?? []), height: Math.min(Math.max(height, CHART_HEIGHT.min), CHART_HEIGHT.max) };
    }
    case "gauge":
      return { kind: "gauge", label: str(value.label), value: num(value.value), min: num(value.min), max: num(value.max), unit: optStr(value.unit) ?? null, color: color(value.color) };
    case "donut": {
      const slices: Slice[] = arr(value.slices).map((entry) => {
        if (!isRecord(entry)) return bad();
        return { label: str(entry.label), value: num(entry.value), ...(typeof entry.color === "string" ? { color: entry.color as ThemeColor } : {}) };
      });
      return { kind: "donut", slices, label: optStr(value.label) ?? null, value: optStr(value.value) ?? null };
    }
    case "heatmap":
      return { kind: "heatmap", values: arr(value.values).map(nums), rows: strs(value.rows), columns: strs(value.columns), color: color(value.color) };
    case "progress":
      return { kind: "progress", label: str(value.label), value: num(value.value), max: num(value.max), detail: optStr(value.detail) ?? null, color: color(value.color) };
    case "button":
      return { kind: "button", label: str(value.label), handler: typeof value.handler === "string" ? value.handler : "", disabled: value.disabled === true };
    case "toggle":
      return { kind: "toggle", label: str(value.label), value: value.value === true, handler: typeof value.handler === "string" ? value.handler : "" };
    case "input":
      return { kind: "input", label: str(value.label), value: typeof value.value === "string" ? value.value : "", placeholder: optStr(value.placeholder) ?? null, handler: typeof value.handler === "string" ? value.handler : "" };
    case "select":
      return { kind: "select", label: str(value.label), options: strs(value.options), selected: Math.round(num(value.selected)), handler: typeof value.handler === "string" ? value.handler : "" };
    case "slider":
      return { kind: "slider", label: str(value.label), value: num(value.value), min: num(value.min), max: num(value.max), step: num(value.step), handler: typeof value.handler === "string" ? value.handler : "" };
    case "tabs":
      return { kind: "tabs", labels: strs(value.labels), selected: Math.round(num(value.selected)), handler: typeof value.handler === "string" ? value.handler : "" };
    case "canvas":
      return { kind: "canvas", width: dimension(value.width), height: dimension(value.height), shapes: arr(value.shapes, MAX_NODES).map(shape) };
    case "scene":
      return { kind: "scene", width: dimension(value.width), height: dimension(value.height) };
    default:
      return null;
  }
}

/** Reads a visual tree out of a document or profile payload; null when it is not a well-formed tree of a known kind. */
export function parseVisualTree(value: unknown): VisualNode | null {
  try {
    return node(value, { nodes: 0 }, 1);
  } catch (failure) {
    if (failure instanceof Malformed) return null;
    throw failure;
  }
}
