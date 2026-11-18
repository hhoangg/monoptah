// F-73, F-74: the desktop's dithered charts (crates/monocode-gpui/src/visuals/charts.rs) as inline SVG, ported from
// mobile/src/features/visuals/charts.tsx at 8b4d76ae. Everything is built from React elements (rects, paths and
// text); no HTML or SVG string is ever parsed. The halftone dither is the desktop's: a tone becomes a share of
// inked 3px cells through a 4x4 Bayer matrix (dither.ts), painted as one `<path>` per colour. Charts draw into a
// fixed viewBox and scale to their container, and they are static (no hover readouts or draw-in motion).
import type { ReactNode, SVGProps } from "react";
import { useAppearance } from "@/lib/appearance";
import { CELL, center, cells, inked, step } from "./dither";
import { Curve, Scale, compact } from "./scale";
import type { Series, Slice, ThemeColor } from "./types";

/** Desktop series hues, tuned for dark and for light surfaces. */
const HUES: Partial<Record<ThemeColor, readonly [dark: string, light: string]>> = {
  positive: ["#4fd57f", "#0a924b"],
  negative: ["#fc5855", "#d01d21"],
  warning: ["#fea247", "#d96700"],
  series2: ["#4fd57f", "#0a924b"],
  series3: ["#b77df8", "#863ad2"],
  series4: ["#fea247", "#d96700"],
  series5: ["#fa78b2", "#d12c80"],
  series6: ["#3faff3", "#0077d7"],
};

/** `color` at `percent` opacity, for any CSS colour (including a `var()`). */
export const alpha = (color: string, percent: number) => `color-mix(in srgb, ${color} ${percent}%, transparent)`;

/** A theme colour token as the desktop resolves it, in this app's tokens. */
export function useVisualColor(): (token?: ThemeColor) => string {
  const dark = useAppearance((state) => state.scheme) === "dark";
  return (token = "accent") => {
    if (token === "ink") return "var(--content)";
    if (token === "muted") return alpha("var(--content)", 60);
    if (token === "faint") return alpha("var(--content)", 20);
    const hue = HUES[token];
    return hue ? hue[dark ? 0 : 1] : "var(--accent-color)";
  };
}

const AXIS = { fill: alpha("var(--content)", 45), fontSize: 10, fontFamily: "ui-monospace, Menlo, monospace" } as const;

/** An SVG that scales to its container's width from a fixed drawing size. */
export function Chart({ width, height, label, children, className }: { width: number; height: number; label: string; children: ReactNode; className?: string }) {
  return (
    <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={label} className={className ?? "block h-auto w-full"} data-visual="chart">
      {children}
    </svg>
  );
}

/** One `<path>` per colour of a cell grid, offset to (x, y). */
function Cells({ paths, x = 0, y = 0 }: { paths: Map<string, string>; x?: number; y?: number }) {
  return (
    <g transform={`translate(${x} ${y})`}>
      {[...paths].map(([color, d]) => (
        <path key={color} d={d} fill={color} />
      ))}
    </g>
  );
}

const BAR_LEVEL = 0.75;
const BAR_FLOOR = 0.125;
const BAR_FADE = 0.7;
const MAX_BAR = 36;
const AREA_LEVEL = 0.625;
const AREA_BANDS: [number, number][] = [
  [0.05, 1],
  [0.13, 0.8],
  [0.25, 0.65],
  [0.4, 0.5],
  [0.6, 0.35],
];
const AREA_REST = 0.2;
const CHART_WIDTH = 600;

/** A line with a soft glow under it, as `glowing_line`. */
function GlowLine({ d, color, x, y, width }: { d: string; color: string; x: number; y: number; width: number }) {
  const common: SVGProps<SVGPathElement> = { d, transform: `translate(${x} ${y})`, stroke: color, fill: "none", strokeLinecap: "round", strokeLinejoin: "round" };
  return (
    <>
      <path {...common} strokeOpacity={0.07} strokeWidth={width + 10} />
      <path {...common} strokeOpacity={0.14} strokeWidth={width + 5} />
      <path {...common} strokeWidth={width} />
    </>
  );
}

/** A square end marker in a faint halo of its colour. */
function Marker({ x, y, color }: { x: number; y: number; color: string }) {
  const side = 2 * CELL;
  return (
    <>
      <rect x={x - (side + 6) / 2} y={y - (side + 6) / 2} width={side + 6} height={side + 6} fill={color} opacity={0.25} />
      <rect x={x - side / 2} y={y - side / 2} width={side} height={side} fill={color} />
    </>
  );
}

/** Bars, lines or areas over a ticked axis, as the desktop's cartesian chart. */
export function Cartesian({ kind, series, labels, height }: { kind: "bar" | "line" | "area"; series: Series[]; labels: string[]; height: number }) {
  const color = useVisualColor();
  const dark = useAppearance((state) => state.scheme) === "dark";
  const width = CHART_WIDTH;
  const bars = kind === "bar";
  const count = Math.max(labels.length, ...series.map((s) => s.values.length), 0);
  if (count === 0) return null;
  const scale = Scale.nice(
    series.flatMap((s) => s.values),
    kind !== "line",
  );
  const ticks = scale.ticks();
  const tickLabels = ticks.map((t) => scale.label(t));
  const longest = Math.max(1, ...tickLabels.map((l) => l.length));
  const left = Math.ceil(longest * 6.1) + 10;
  const top = bars ? 16 : 8;
  const right = width - (bars ? 0 : 6);
  const cols = Math.max(0, Math.floor((right - left) / CELL));
  const rows = Math.max(0, Math.floor((height - top - 22) / CELL));
  const plotW = cols * CELL;
  const plotH = rows * CELL;
  const yOf = (v: number) => scale.depth(v) * plotH;
  const band = plotW / count;
  const xOf = (i: number) => (bars ? band * (i + 0.5) : count <= 1 ? plotW / 2 : (i * plotW) / (count - 1));
  const tints = series.map((s) => color(s.color ?? "series1"));
  const tintOf = (index: number) => tints[index] ?? "var(--accent-color)";
  const stride = Math.max(1, Math.ceil(count / Math.max(1, Math.floor(plotW / 72))));

  let marks: ReactNode = null;
  let lastValue: ReactNode = null;
  if (bars) {
    const n = Math.max(1, series.length);
    const widthPx = Math.min((band * 0.7 - CELL * (n - 1)) / n, MAX_BAR);
    const wide = Math.max(1, Math.round(widthPx / CELL));
    const group = wide * n + (n - 1);
    const rowOf = (v: number) => Math.round((scale.depth(v) * plotH) / CELL);
    const zero = rowOf(Math.min(Math.max(scale.min, 0), scale.max));
    const fade = Math.max(1, rows * BAR_FADE);
    const columns: ([number, number, number] | null)[] = new Array<[number, number, number] | null>(cols).fill(null);
    let lastTop: number | null = null;
    for (let i = 0; i < count; i++) {
      const start = Math.round((band * (i + 0.5)) / CELL - group / 2);
      series.forEach((s, j) => {
        const value = s.values[i];
        if (value === undefined) return;
        const end = rowOf(value);
        const first = start + j * (wide + 1);
        for (let col = first; col < first + wide; col++) if (col >= 0 && col < cols) columns[col] = [Math.min(end, zero), Math.max(end, zero), j];
        if (i === count - 1) lastTop = Math.min(end, zero);
      });
    }
    marks = (
      <Cells
        x={left}
        y={top}
        paths={cells(cols, rows, (col, row) => {
          const bar = columns[col];
          if (!bar || row < bar[0] || row >= bar[1]) return null;
          const tone = BAR_LEVEL - (BAR_LEVEL - BAR_FLOOR) * Math.min((row - bar[0]) / fade, 1);
          return row === bar[0] || inked(col, row, tone) ? tintOf(bar[2]) : null;
        })}
      />
    );
    // With one series, the latest value is written over its bar.
    const latest = series[0]?.values[count - 1];
    if (series.length === 1 && lastTop !== null && latest !== undefined)
      lastValue = (
        <text {...AXIS} fill={alpha("var(--content)", 90)} x={left + band * (count - 0.5)} y={top + lastTop * CELL - 5} textAnchor="middle">
          {compact(latest, scale.step / 10)}
        </text>
      );
  } else {
    const curves = series.map((s) => new Curve(s.values.map((v, i) => [xOf(i), yOf(v)] as [number, number])));
    const area =
      kind === "area"
        ? (() => {
            const baseline = yOf(Math.max(scale.min, 0));
            return (
              <>
                {curves.map((curve, index) => (
                  <path
                    key={`wash${index}`}
                    transform={`translate(${left} ${top})`}
                    d={`${curve.path(4)}L${curve.lastX} ${baseline}L${curve.firstX} ${baseline}z`}
                    fill={tintOf(index)}
                    fillOpacity={dark ? 0.07 : 0.04}
                  />
                ))}
                <Cells
                  x={left}
                  y={top}
                  paths={cells(cols, rows, (col, row) => {
                    const [x, y] = center(col, row);
                    if (y > baseline) return null;
                    // Later series sit in front, as stacked paper would.
                    for (let index = curves.length - 1; index >= 0; index--) {
                      const curve = curves[index];
                      if (!curve || x < curve.firstX || x > curve.lastX || y < curve.at(x)) continue;
                      const depth = (y - curve.at(x)) / Math.max(plotH, 1);
                      const lead = Math.max((2 * CELL) / Math.max(plotH, 1), AREA_BANDS[0]?.[0] ?? 0.05);
                      const share = depth < lead ? 1 : (AREA_BANDS.find(([d]) => depth < d)?.[1] ?? AREA_REST);
                      return inked(col, row, step(AREA_LEVEL * share)) ? tintOf(index) : null;
                    }
                    return null;
                  })}
                />
              </>
            );
          })()
        : null;
    marks = (
      <>
        {area}
        {curves.map((curve, index) => (
          <GlowLine key={`line${index}`} d={curve.path(2)} color={tintOf(index)} x={left} y={top} width={2} />
        ))}
        {series.map((s, index) => {
          const last = s.values.length - 1;
          const value = s.values[last];
          if (last < 0 || value === undefined) return null;
          return <Marker key={`end${index}`} x={left + xOf(last)} y={top + yOf(value)} color={tintOf(index)} />;
        })}
      </>
    );
  }
  return (
    <Chart width={width} height={height} label={`${kind} chart`}>
      {ticks.map((tick, index) => {
        const y = top + Math.round(yOf(tick));
        return (
          <g key={tick}>
            <rect x={left} y={y} width={plotW} height={1} fill={alpha("var(--content)", 7)} />
            <text {...AXIS} x={left - 10} y={y + 3.5} textAnchor="end">
              {tickLabels[index]}
            </text>
          </g>
        );
      })}
      {labels.map((label, i) =>
        i % stride ? null : (
          <text key={i} {...AXIS} x={left + xOf(i)} y={top + plotH + 16} textAnchor={bars ? "middle" : i === 0 ? "start" : i + 1 === count ? "end" : "middle"}>
            {label.length > 12 ? `${label.slice(0, 11)}...` : label}
          </text>
        ),
      )}
      {marks}
      {lastValue}
    </Chart>
  );
}

/** The sparkline under a stat: a line over bands that step lighter downward. */
export function Trend({ values, tint }: { values: number[]; tint: string }) {
  const WIDTH = 240;
  const HEIGHT = 56;
  const LEVELS = [10 / 16, 6 / 16, 3 / 16, 1 / 16];
  if (values.length < 2) return null;
  const cols = Math.floor(WIDTH / CELL);
  const rows = Math.floor(HEIGHT / CELL);
  const w = cols * CELL;
  const h = rows * CELL;
  const low = Math.min(...values);
  const high = Math.max(...values);
  const range = high > low ? high - low : 1;
  const top = 6;
  const floor = h * 0.3;
  const curve = new Curve(values.map((v, i) => [(i / (values.length - 1)) * w, top + (1 - (v - low) / range) * (h - top - floor)]));
  return (
    <Chart width={WIDTH} height={HEIGHT} label="Trend" className="block h-auto w-full max-w-60">
      <Cells
        paths={cells(cols, rows, (col, row) => {
          const [x, y] = center(col, row);
          const depth = y - curve.at(x);
          if (depth < 0) return null;
          return inked(col, row, LEVELS[Math.min(Math.floor(depth / 7), LEVELS.length - 1)] ?? 0) ? tint : null;
        })}
      />
      <GlowLine d={curve.path(2)} color={tint} x={0} y={0} width={1.5} />
      <Marker x={w - CELL} y={curve.at(w)} color={tint} />
    </Chart>
  );
}

/** A radial dial of dithered blocks. */
export function Gauge({ label, value, min, max, unit, tint }: { label: string; value: number; min: number; max: number; unit?: string | null; tint: string }) {
  const SEGMENTS = 36;
  const SPAN = 270;
  const START = -135;
  const side = 220;
  const ratio = Math.min(Math.max((value - min) / (max - min || 1), 0), 1);
  const format = (v: number) => (unit === "%" ? `${compact(v, 1)}%` : unit ? `${compact(v, 0.1)} ${unit}` : compact(v, 0.1));
  const outerFull = side / 2 - 1 - CELL;
  const height = Math.ceil(side / 2 + (outerFull + CELL) * Math.SQRT1_2 + 18);
  const c = side / 2;
  const outer = side / 2 - 1 - CELL;
  const thickness = Math.max(2, Math.round((side * 0.12) / CELL)) * CELL;
  const inner = outer - thickness;
  const segment = SPAN / SEGMENTS;
  const middle = outer - thickness / 2;
  const gap = ((2 / middle) * 180) / Math.PI;
  const filled = ratio * SEGMENTS;
  const active = Math.max(0, Math.ceil(filled - 1e-6));
  const track = alpha(tint, 14);
  const paths = cells(Math.floor(side / CELL), Math.floor(height / CELL), (col, row) => {
    const [x, y] = center(col, row);
    const dx = x - c;
    const dy = y - c;
    const r = Math.hypot(dx, dy);
    if (r < inner || r > outer + CELL) return null;
    const along = (Math.atan2(dx, -dy) * 180) / Math.PI - START;
    if (along < 0 || along >= SPAN) return null;
    const index = Math.floor(along / segment);
    const within = along - index * segment;
    if (within < gap / 2 || within > segment - gap / 2) return null;
    const head = active > 0 && index === active - 1;
    if (r > outer && !head) return null;
    if (index >= active) return track;
    if (head) {
      const part = Math.min(Math.max(filled - index, 0), 1);
      if (within > gap / 2 + (segment - gap) * Math.max(part, 0.15)) return r <= outer ? track : null;
      return tint;
    }
    const tone = Math.max(1 - (active - 1 - index) / 16, 9 / 16);
    return inked(col, row, tone) ? tint : null;
  });
  const ends = middle * Math.SQRT1_2;
  return (
    <Chart width={side} height={height} label={`${label}: ${format(value)}`} className="mx-auto block h-auto w-full max-w-[220px]">
      <Cells paths={paths} />
      <text x={c} y={c - 4 + side * 0.05} fontSize={Math.round(side * 0.15)} fontWeight="600" fill="var(--content)" textAnchor="middle">
        {format(value)}
      </text>
      <text x={c} y={c + side * 0.1 + 10} fontSize={12} fill={alpha("var(--content)", 60)} textAnchor="middle">
        {label}
      </text>
      {(
        [
          [-ends, min],
          [ends, max],
        ] as const
      ).map(([dx, v]) => (
        <text key={dx} {...AXIS} x={c + dx} y={c + ends + thickness / 2 + 15} textAnchor="middle">
          {compact(v, 1)}
        </text>
      ))}
    </Chart>
  );
}

/** Parts of a whole as a dithered ring with a legend. */
export function Donut({ slices, label, value }: { slices: Slice[]; label?: string | null; value?: string | null }) {
  const color = useVisualColor();
  const SIZE = 184;
  const FLOOR = 3 / 16;
  const total = slices.reduce((sum, s) => sum + Math.max(s.value, 0), 0);
  const figure = value ?? (Math.abs(total) >= 1e4 ? Math.round(total).toLocaleString() : compact(total, 0.1));
  const tints = slices.map((s) => color(s.color ?? "series1"));
  const grid = Math.floor(SIZE / CELL);
  const half = (grid * CELL) / 2;
  const outer = half - 2 * CELL;
  const inner = outer - Math.round((SIZE * 0.17) / CELL) * CELL;
  const TAU = Math.PI * 2;
  const angleOf = (dx: number, dy: number) => {
    const a = Math.atan2(dx, -dy);
    return a < 0 ? a + TAU : a;
  };
  const edges: number[] = [];
  let running = 0;
  for (const s of slices) {
    edges.push((running / Math.max(total, 1e-9)) * TAU);
    running += Math.max(s.value, 0);
  }
  const shown = slices.filter((s) => s.value > 0).length;
  const paths = cells(grid, grid, (col, row) => {
    const [x, y] = center(col, row);
    const dx = x - half;
    const dy = y - half;
    const r = Math.hypot(dx, dy);
    if (r < inner || r > outer) return null;
    const angle = angleOf(dx, dy);
    let index = -1;
    for (let i = edges.length - 1; i >= 0; i--)
      if (angle >= (edges[i] ?? 0)) {
        index = i;
        break;
      }
    if (index < 0) return null;
    let outline = r > outer - CELL || r < inner + CELL;
    if (shown > 1) {
      let near = Infinity;
      edges.forEach((edge, i) => {
        if ((slices[i]?.value ?? 0) <= 0) return;
        let d = Math.abs(angle - edge);
        d = Math.min(d, TAU - d);
        if (d <= Math.PI / 2) near = Math.min(near, (r * Math.sin(d)) / CELL);
      });
      if (near < 0.75) return null;
      outline ||= near < 1.75;
    }
    const depth = (outer - r) / (outer - inner);
    const tone = FLOOR + (1 - FLOOR) * (1 - depth) ** 1.4;
    return outline || inked(col, row, tone) ? (tints[index] ?? null) : null;
  });
  return (
    <div className="flex flex-wrap items-center gap-4">
      <Chart width={SIZE} height={SIZE} label={`Donut${label ? `: ${label}` : ""}`} className="block h-auto w-[184px] max-w-full shrink-0">
        <Cells paths={paths} />
        {label ? (
          <text {...AXIS} x={half} y={half - 12} textAnchor="middle">
            {label.toUpperCase().slice(0, 18)}
          </text>
        ) : null}
        <text
          x={half}
          y={half + (label ? 12 : 8)}
          fontSize={Math.floor(Math.min(26, (inner * 2 - 16) / (Math.max(figure.length, 1) * 0.62)))}
          fontWeight="600"
          fill="var(--content)"
          textAnchor="middle"
        >
          {figure}
        </text>
      </Chart>
      <ul className="flex min-w-0 flex-1 flex-col gap-1.5">
        {slices.map((s, index) => (
          <li key={`${s.label}${index}`} className="flex items-center gap-2 text-small">
            <span aria-hidden="true" className="size-2.5 shrink-0 rounded-[3px]" style={{ background: tints[index] }} />
            <span className="min-w-0 flex-1 truncate text-[color:var(--content-a85)]">{s.label}</span>
            <span className="text-muted-text tabular-nums">
              {compact(s.value, 0.1)} · {(total > 0 ? (s.value / total) * 100 : 0).toFixed(0)}%
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Values by row and column as dithered squares, as the desktop's heatmap. */
export function Heatmap({ values, rows, columns, tint }: { values: number[][]; rows: string[]; columns: string[]; tint: string }) {
  const SQUARE = 4;
  const PITCH = 5;
  const AVAILABLE = CHART_WIDTH;
  const width = values[0]?.length ?? 0;
  const gutter = rows.length ? 34 : 0;
  const top = columns.length ? 18 : 0;
  const height = top + values.length * PITCH * CELL;
  const high = Math.max(1e-9, ...values.flat());
  const track = alpha("var(--content)", 7);
  const cols = Math.floor((AVAILABLE - gutter) / CELL);
  const fits = Math.max(1, Math.floor((cols + PITCH - SQUARE) / PITCH));
  const skip = Math.max(0, width - fits);
  const pitch = PITCH * CELL;
  const paths = cells(cols, values.length * PITCH, (col, row) => {
    const x = col % PITCH;
    const y = row % PITCH;
    if (x >= SQUARE || y >= SQUARE || Math.floor(col / PITCH) >= fits) return null;
    const v = values[Math.floor(row / PITCH)]?.[Math.floor(col / PITCH) + skip];
    if (v === undefined) return null;
    if (v <= 0) return track;
    return inked(x, y, step(0.25 + 0.75 * (v / high))) ? tint : track;
  });
  let previous: [string, number] | null = null;
  return (
    <Chart width={AVAILABLE} height={height} label="Heatmap">
      <Cells paths={paths} x={gutter} y={top} />
      {rows.map((label, i) =>
        rows.length > 4 && i % 2 === 1 ? null : (
          <text key={i} {...AXIS} x={0} y={top + i * pitch + (SQUARE * CELL) / 2 + 3.5}>
            {label}
          </text>
        ),
      )}
      {columns.slice(skip).map((label, i) => {
        const x = i * pitch;
        if (previous && (previous[0] === label || x - previous[1] < 36)) return null;
        previous = [label, x];
        return (
          <text key={i} {...AXIS} x={gutter + x} y={top / 2 + 3.5}>
            {label}
          </text>
        );
      })}
    </Chart>
  );
}

/** A labelled bar of dithered cells, denser toward its head. */
export function Progress({ label, value, max, detail, tint }: { label: string; value: number; max: number; detail?: string | null; tint: string }) {
  const ratio = Math.min(Math.max(value / (max || 1), 0), 1);
  const width = CHART_WIDTH;
  const cols = Math.floor(width / CELL);
  const head = Math.round(cols * ratio);
  const track = alpha(tint, 14);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between gap-2">
        <span className="min-w-0 truncate text-small">{label}</span>
        <span className="text-meta text-muted-text tabular-nums">{(ratio * 100).toFixed(0)}%</span>
      </div>
      <Chart width={width} height={3 * CELL} label={`${label}: ${(ratio * 100).toFixed(0)}%`}>
        <Cells
          paths={cells(cols, 3, (col, row) => {
            if (col >= head) return track;
            const tone = 0.3 + 0.7 * (col / Math.max(head, 1)) ** 1.5;
            return col >= head - 3 || inked(col, row, tone) ? tint : track;
          })}
        />
      </Chart>
      {detail ? <span className="text-meta text-muted-text">{detail}</span> : null}
    </div>
  );
}

/**
 * A Mono's contributions as the desktop's dashboard draws them (F-74): weeks as columns, Sunday to Saturday down,
 * each day a square of 4x4 dither cells in the accent, denser the busier it was. `levels` run day by day from a
 * Sunday (26 weeks), each 0 to 4.
 */
export function ContributionGraph({ levels, today = new Date() }: { levels: number[]; today?: Date }) {
  const TONES = [0, 0.25, 0.5, 0.75, 1];
  const SQUARE = 4;
  const PITCH = 5;
  const GUTTER = 28;
  const MONTHS = 18;
  const width = GUTTER + Math.ceil(levels.length / 7) * PITCH * CELL;
  const height = MONTHS + 7 * PITCH * CELL;
  const weeks = Math.ceil(levels.length / 7);
  // The first day shown: today, back as many days as the graph has.
  const start = new Date(today);
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - (levels.length - 1));
  const pitch = PITCH * CELL;
  const track = alpha("var(--content)", 7);
  const paths = cells(weeks * PITCH, 7 * PITCH, (col, row) => {
    const x = col % PITCH;
    const y = row % PITCH;
    const c = Math.floor(col / PITCH);
    const r = Math.floor(row / PITCH);
    if (x >= SQUARE || y >= SQUARE) return null;
    const level = levels[c * 7 + r];
    if (level === undefined) return null;
    if (level === 0) return track;
    return inked(x, y, TONES[Math.min(level, 4)] ?? 1) ? "var(--accent-color)" : track;
  });
  const months: ReactNode[] = [];
  let lastAt = -Infinity;
  for (let c = 0; c < weeks; c++) {
    const week = new Date(start);
    week.setDate(start.getDate() + c * 7);
    const x = c * pitch;
    if ((c === 0 || week.getDate() <= 7) && x - lastAt >= 30) {
      lastAt = x;
      months.push(
        <text key={c} {...AXIS} x={GUTTER + x} y={MONTHS / 2 + 3.5}>
          {week.toLocaleDateString([], { month: "short" })}
        </text>,
      );
    }
  }
  return (
    <Chart width={width} height={height} label="Contributions over the last 6 months">
      <Cells paths={paths} x={GUTTER} y={MONTHS} />
      {(
        [
          [1, "Mon"],
          [3, "Wed"],
          [5, "Fri"],
        ] as const
      ).map(([r, name]) => (
        <text key={name} {...AXIS} x={0} y={MONTHS + r * pitch + (SQUARE * CELL) / 2 + 3.5}>
          {name}
        </text>
      ))}
      {months}
    </Chart>
  );
}
