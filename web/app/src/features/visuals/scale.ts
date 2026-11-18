// Copied from mobile/src/features/visuals/scale.ts at 8b4d76ae (adapted for noUncheckedIndexedAccess and erasableSyntaxOnly).
// The desktop charts' axis scale and curve (visuals/dither.rs), so a web chart ticks and bends as the desktop's does.

const at = (list: readonly number[], index: number): number => list[index] ?? 0;

export class Scale {
  readonly min: number;
  readonly max: number;
  readonly step: number;

  constructor(min: number, max: number, step: number) {
    this.min = min;
    this.max = max;
    this.step = step;
  }

  /** A range of round steps around `values`, from zero when `zero`. */
  static nice(values: number[], zero: boolean) {
    let low = zero ? 0 : Infinity;
    let high = zero ? 0 : -Infinity;
    for (const v of values) {
      if (!Number.isFinite(v)) continue;
      low = Math.min(low, v);
      high = Math.max(high, v);
    }
    if (low > high) [low, high] = [0, 1];
    const span = high - low > 0 ? high - low : Math.abs(high) > 0 ? Math.abs(high) : 1;
    const raw = span / 4;
    const magnitude = 10 ** Math.floor(Math.log10(raw));
    const step = ([1, 2, 2.5, 5, 10].find((c) => c * magnitude >= raw) ?? 10) * magnitude;
    const min = Math.floor(low / step) * step;
    let max = Math.ceil(high / step) * step;
    // A flat series still needs a range to sit in.
    if (max <= min) max = min + step;
    return new Scale(min, max, step);
  }

  ticks() {
    const count = Math.min(12, Math.round((this.max - this.min) / this.step));
    return Array.from({ length: count + 1 }, (_, i) => this.min + this.step * i);
  }

  /** Where `value` falls, 0 at `max` and 1 at `min`. */
  depth(value: number) {
    return (this.max - value) / (this.max - this.min);
  }

  label(value: number) {
    return compact(value, this.step);
  }
}

/** 1.2K, 48K, 3.4M; plain numbers keep `step`'s decimals. */
export function compact(value: number, step: number) {
  const trim = (s: string) => (s.includes(".") ? s.replace(/0+$/, "").replace(/\.$/, "") : s);
  const abs = Math.abs(value);
  if (abs >= 1e9) return `${trim((value / 1e9).toFixed(1))}B`;
  if (abs >= 1e6) return `${trim((value / 1e6).toFixed(1))}M`;
  if (abs >= 1e4) return `${trim((value / 1e3).toFixed(1))}K`;
  let decimals = 4;
  for (let d = 0; d < 4; d++) {
    const scaled = Math.abs(step) * 10 ** d;
    if (Math.abs(scaled - Math.round(scaled)) < 1e-6) {
      decimals = d;
      break;
    }
  }
  return trim(value.toFixed(decimals));
}

/** A smooth curve through points that never overshoots them (monotone cubic,
 *  Fritsch-Carlson), so peaks stay at the data. */
export class Curve {
  private xs: number[];
  private ys: number[];
  private ms: number[];

  constructor(points: [number, number][]) {
    this.xs = points.map((p) => p[0]);
    this.ys = points.map((p) => p[1]);
    const n = points.length;
    const ms = new Array<number>(n).fill(0);
    if (n >= 2) {
      const d = Array.from({ length: n - 1 }, (_, k) => (at(this.ys, k + 1) - at(this.ys, k)) / Math.max(at(this.xs, k + 1) - at(this.xs, k), 1e-6));
      ms[0] = at(d, 0);
      ms[n - 1] = at(d, n - 2);
      for (let k = 1; k < n - 1; k++) ms[k] = at(d, k - 1) * at(d, k) <= 0 ? 0 : (at(d, k - 1) + at(d, k)) / 2;
      for (let k = 0; k < n - 1; k++) {
        const dk = at(d, k);
        if (dk === 0) {
          ms[k] = 0;
          ms[k + 1] = 0;
          continue;
        }
        const a = at(ms, k) / dk;
        const b = at(ms, k + 1) / dk;
        const s = a * a + b * b;
        if (s > 9) {
          const t = 3 / Math.sqrt(s);
          ms[k] = t * a * dk;
          ms[k + 1] = t * b * dk;
        }
      }
    }
    this.ms = ms;
  }

  get firstX() {
    return this.xs[0] ?? 0;
  }
  get lastX() {
    return this.xs[this.xs.length - 1] ?? 0;
  }

  /** The curve's y at `x`, clamped to its ends. */
  at(x: number) {
    const { xs, ys, ms } = this;
    const n = xs.length;
    if (n === 0) return 0;
    if (n === 1 || x <= at(xs, 0)) return at(ys, 0);
    if (x >= at(xs, n - 1)) return at(ys, n - 1);
    let k = 0;
    while (k < n - 2 && at(xs, k + 1) <= x) k++;
    const h = Math.max(at(xs, k + 1) - at(xs, k), 1e-6);
    const t = (x - at(xs, k)) / h;
    const t2 = t * t;
    const t3 = t2 * t;
    return (2 * t3 - 3 * t2 + 1) * at(ys, k) + (t3 - 2 * t2 + t) * h * at(ms, k) + (-2 * t3 + 3 * t2) * at(ys, k + 1) + (t3 - t2) * h * at(ms, k + 1);
  }

  /** Points along the curve every `step` points, as an SVG path. */
  path(step = 2) {
    const a = this.firstX;
    const b = this.lastX;
    if (this.xs.length < 2) return this.xs.length ? `M${a} ${at(this.ys, 0)}` : "";
    const count = Math.max(1, Math.ceil((b - a) / step));
    let d = "";
    for (let i = 0; i <= count; i++) {
      const x = a + ((b - a) * i) / count;
      d += `${i ? "L" : "M"}${x.toFixed(2)} ${this.at(x).toFixed(2)}`;
    }
    return d;
  }
}
