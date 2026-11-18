// F-73: a visual's component tree drawn as React elements and inline SVG, as the desktop's VisualView lays it
// out (ported from mobile/src/features/visuals/VisualTree.tsx at 8b4d76ae). The tree is data from the desktop,
// never HTML: text goes through React children, so a hostile string is only text. Interactive nodes (button,
// toggle, input, select, slider, tabs) show their state but do not act: the visual runs on the desktop. 3D
// scenes are not drawn.
import { Component, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Cartesian, Donut, Gauge, Heatmap, Progress, Trend, alpha, useVisualColor } from "./charts";
import type { Shape, TextSize, VisualNode } from "./types";

export { parseVisualTree } from "./parse";

const TEXT_CLASS: Record<TextSize, string> = {
  caption: "text-[11px] leading-4",
  small: "text-small",
  body: "text-body",
  subtitle: "text-[17px] leading-6 font-semibold",
  title: "text-[22px] leading-7 font-semibold",
  hero: "text-[34px] leading-10 font-semibold",
};

/** A node the renderer cannot draw (data the parser let through but a chart cannot lay out) must not take the page down. */
class VisualBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  override state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  override render() {
    return this.state.failed ? <p className="text-small text-muted-text">This visual could not be drawn. Open it on your desktop to see it.</p> : this.props.children;
  }
}

export function VisualTree({ node }: { node: VisualNode }) {
  return (
    <div data-testid="visual-tree" className="min-w-0">
      <VisualBoundary>
        <Node node={node} />
      </VisualBoundary>
    </div>
  );
}

function Node({ node }: { node: VisualNode }) {
  const color = useVisualColor();
  switch (node.kind) {
    case "column":
    case "card":
      return (
        <div className={cn("flex flex-col", node.kind === "card" && "rounded-card border border-stroke bg-[var(--content-a04)] p-4")} style={{ gap: node.gap }}>
          {node.children.map((child, i) => (
            <Node key={i} node={child} />
          ))}
        </div>
      );
    case "row":
      return (
        <div className="flex flex-wrap items-start" style={{ gap: node.gap }}>
          {node.children.map((child, i) => (
            <div key={i} className="min-w-[120px] flex-1">
              <Node node={child} />
            </div>
          ))}
        </div>
      );
    case "grid":
      return (
        <div className="grid" style={{ gap: node.gap, gridTemplateColumns: `repeat(auto-fit, minmax(min(100%, 180px), 1fr))` }} data-columns={node.columns}>
          {node.children.map((child, i) => (
            <div key={i} className="min-w-0">
              <Node node={child} />
            </div>
          ))}
        </div>
      );
    case "text":
      return (
        <p className={cn("wrap-anywhere", TEXT_CLASS[node.size] ?? "text-body")} style={{ color: color(node.color ?? "ink") }}>
          {node.text}
        </p>
      );
    case "stat": {
      let delta = node.delta ?? null;
      let detail = node.detail ?? null;
      // "+12.4% vs last month" lends its leading change to the delta.
      if (!delta && detail) {
        const match = /^([+\-−]\d\S*)\s*(.*)$/.exec(detail);
        if (match) {
          delta = match[1] ?? null;
          detail = match[2] || null;
        }
      }
      const tint = ["ink", "muted", "faint"].includes(node.color) ? "var(--accent-color)" : color(node.color);
      const down = !!delta && (delta.startsWith("-") || delta.startsWith("−"));
      return (
        <div className="flex flex-col gap-1">
          <span className="truncate text-small text-muted-text">{node.label}</span>
          <span className="text-[30px] leading-[38px] font-semibold tabular-nums" style={{ color: color(node.color ?? "ink") }}>
            {node.value}
          </span>
          {delta || detail ? (
            <span className="flex flex-wrap items-center gap-x-2 text-meta text-muted-text">
              {delta ? (
                <span className="font-mono" style={{ color: color(down !== !!node.lower_is_better ? "negative" : "positive") }}>
                  {down ? "▼" : "▲"} {delta}
                </span>
              ) : null}
              {detail ? <span>{detail}</span> : null}
            </span>
          ) : null}
          {node.trend && node.trend.length > 1 ? (
            <div className="pt-2">
              <Trend values={node.trend} tint={tint} />
            </div>
          ) : null}
        </div>
      );
    }
    case "table":
      return (
        <div className="overflow-x-auto">
          <table className="border-collapse text-small">
            <thead>
              <tr>
                {node.columns.map((column, c) => (
                  <th key={c} scope="col" className="min-w-[110px] border-b border-stroke px-2 py-2 text-left font-semibold text-muted-text">
                    {column}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {node.rows.map((cells, r) => (
                <tr key={r}>
                  {cells.map((cell, c) => (
                    <td key={c} className="min-w-[110px] border-b border-stroke px-2 py-2">
                      {cell}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "chart":
      return <Cartesian kind={node.chart_type} series={node.series} labels={node.labels} height={node.height || 220} />;
    case "gauge":
      return <Gauge label={node.label} value={node.value} min={node.min} max={node.max} unit={node.unit ?? null} tint={color(node.color)} />;
    case "donut":
      return <Donut slices={node.slices} label={node.label ?? null} value={node.value ?? null} />;
    case "heatmap":
      return <Heatmap values={node.values} rows={node.rows} columns={node.columns} tint={color(node.color)} />;
    case "progress":
      return <Progress label={node.label} value={node.value} max={node.max} detail={node.detail ?? null} tint={color(node.color)} />;
    case "button":
      return (
        <span className={cn("inline-flex h-8 items-center self-start rounded-row bg-[var(--content-a08)] px-3 text-small", node.disabled && "opacity-40")} data-visual="button">
          {node.label}
        </span>
      );
    case "toggle":
      return (
        <div className="flex items-center justify-between gap-3 text-small" data-visual="toggle">
          <span className="min-w-0 flex-1">{node.label}</span>
          <span aria-hidden="true" className="inline-flex h-5 w-[34px] items-center rounded-pill p-0.5" style={{ background: node.value ? "var(--accent-color)" : alpha("var(--content)", 15) }}>
            <span className={cn("size-4 rounded-pill bg-inverse", node.value && "ml-3.5")} />
          </span>
          <span className="sr-only">{node.value ? "On" : "Off"}</span>
        </div>
      );
    case "input":
      return (
        <div className="flex flex-col gap-1.5" data-visual="input">
          <span className="text-meta text-muted-text">{node.label}</span>
          <span className="rounded-row border border-stroke px-2.5 py-2 text-small">{node.value || <span className="text-muted-text">{node.placeholder ?? ""}</span>}</span>
        </div>
      );
    case "select":
      return (
        <div className="flex items-center justify-between gap-3 text-small" data-visual="select">
          <span className="min-w-0 flex-1">{node.label}</span>
          <span className="text-muted-text">{node.options[node.selected] ?? ""}</span>
        </div>
      );
    case "slider": {
      const share = Math.min(Math.max((node.value - node.min) / (node.max - node.min || 1), 0), 1);
      return (
        <div className="flex flex-col gap-1.5" data-visual="slider">
          <div className="flex items-center justify-between gap-3 text-small">
            <span className="min-w-0 flex-1">{node.label}</span>
            <span className="font-mono text-meta text-muted-text">{node.value}</span>
          </div>
          <div className="h-1 overflow-hidden rounded-pill bg-[var(--content-a08)]">
            <div className="h-1" style={{ width: `${share * 100}%`, background: "var(--accent-color)" }} />
          </div>
        </div>
      );
    }
    case "tabs":
      return (
        <div className="flex flex-wrap gap-1 self-start rounded-row bg-[var(--content-a05)] p-[3px]" data-visual="tabs">
          {node.labels.map((label, i) => (
            <span key={`${label}${i}`} className={cn("rounded-[8px] px-2.5 py-1 text-small text-muted-text", i === node.selected && "bg-[var(--content-a08)] text-content")}>
              {label}
            </span>
          ))}
        </div>
      );
    case "canvas":
      return (
        <svg viewBox={`0 0 ${node.width} ${node.height}`} role="img" aria-label="Canvas" className="block h-auto w-full" data-visual="canvas">
          {node.shapes.map((shape, i) => (
            <CanvasShape key={i} shape={shape} tint={color(shape.color)} />
          ))}
        </svg>
      );
    case "scene":
      return (
        <div className="grid place-items-center rounded-card border border-stroke bg-[var(--content-a04)] p-4 text-small text-muted-text" style={{ aspectRatio: node.width / node.height || 16 / 9 }}>
          Open this 3D scene on your desktop to see it.
        </div>
      );
    default:
      return null;
  }
}

function CanvasShape({ shape, tint }: { shape: Shape; tint: string }) {
  switch (shape.kind) {
    case "rect":
      return <rect x={shape.x} y={shape.y} width={shape.width} height={shape.height} fill={tint} />;
    case "circle":
      return <circle cx={shape.x} cy={shape.y} r={shape.radius} fill={tint} />;
    case "line":
      return <line x1={shape.x1} y1={shape.y1} x2={shape.x2} y2={shape.y2} stroke={tint} strokeWidth={shape.width} />;
    case "polyline":
      return <polyline points={shape.points.map((point) => point.join(",")).join(" ")} stroke={tint} strokeWidth={shape.width} fill="none" />;
    case "label":
      return (
        <text x={shape.x} y={shape.y} fill={tint} fontSize={12}>
          {shape.text}
        </text>
      );
  }
}
