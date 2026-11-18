// WCAG 2.x contrast helpers and a small resolver for the design tokens in styles/tokens.css, so the
// contrast of every theme, scheme and accent can be measured from the real CSS (see contrast.test.ts).

export interface Rgba {
  r: number; // 0-255
  g: number;
  b: number;
  a: number; // 0-1
}

export function parseColor(input: string): Rgba {
  const value = input.trim().toLowerCase();
  if (value === "black") return { r: 0, g: 0, b: 0, a: 1 };
  if (value === "white") return { r: 255, g: 255, b: 255, a: 1 };
  if (value === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/.exec(value);
  if (hex) {
    const digits = hex[1] ?? "";
    const h = digits.length === 3 ? [...digits].map((c) => c + c).join("") : digits;
    return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16), a: 1 };
  }
  const fn = /^rgba?\(([^)]+)\)$/.exec(value);
  if (fn) {
    const [r, g, b, a] = (fn[1] ?? "").split(",").map((p) => Number.parseFloat(p));
    if (r !== undefined && g !== undefined && b !== undefined && [r, g, b, a ?? 1].every((n) => Number.isFinite(n))) return { r, g, b, a: a ?? 1 };
  }
  throw new Error(`Unsupported colour: ${input}`);
}

/** Alpha-composites `top` over an opaque `bottom`. */
export function composite(top: Rgba, bottom: Rgba): Rgba {
  const a = top.a;
  return { r: top.r * a + bottom.r * (1 - a), g: top.g * a + bottom.g * (1 - a), b: top.b * a + bottom.b * (1 - a), a: 1 };
}

function channel(value: number): number {
  const s = value / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

/** WCAG relative luminance of an opaque colour. */
export function relativeLuminance(color: Rgba): number {
  return 0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b);
}

/** WCAG contrast ratio (1 to 21). A translucent foreground is composited over the background first. */
export function contrastRatio(foreground: string | Rgba, background: string | Rgba): number {
  const bg = typeof background === "string" ? parseColor(background) : background;
  const fgRaw = typeof foreground === "string" ? parseColor(foreground) : foreground;
  const fg = fgRaw.a < 1 ? composite(fgRaw, bg) : fgRaw;
  const l1 = relativeLuminance(fg);
  const l2 = relativeLuminance(bg);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

export interface Selection {
  scheme: "light" | "dark";
  theme: string;
  accent: string;
}

export interface Rule {
  attrs: Record<string, string>;
  decls: Record<string, string>;
  order: number;
}

/** Parses the flat `:root[...] { --x: y; }` rules of tokens.css and contrast-overrides.css. */
export function parseRules(css: string, startOrder = 0): Rule[] {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "").replace(/@media[^{]*\{[\s\S]*?\}\s*\}/g, "");
  const rules: Rule[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let match: RegExpExecArray | null;
  let order = startOrder;
  while ((match = re.exec(stripped))) {
    const selectors = (match[1] ?? "").split(",").map((s) => s.trim());
    const decls: Record<string, string> = {};
    for (const decl of (match[2] ?? "").split(";")) {
      const idx = decl.indexOf(":");
      if (idx < 0) continue;
      const name = decl.slice(0, idx).trim();
      if (name.startsWith("--")) decls[name] = decl.slice(idx + 1).trim();
    }
    for (const selector of selectors) {
      if (!selector.startsWith(":root")) continue;
      const attrs: Record<string, string> = {};
      for (const attr of selector.matchAll(/\[data-([a-z]+)="([^"]+)"\]/g)) attrs[attr[1] ?? ""] = attr[2] ?? "";
      rules.push({ attrs, decls, order: order++ });
    }
  }
  return rules;
}

/** The custom properties that apply to <html data-scheme data-theme data-accent>, by specificity then source order. */
export function computeVars(rules: readonly Rule[], sel: Selection): Record<string, string> {
  const current: Record<string, string> = { scheme: sel.scheme, theme: sel.theme, accent: sel.accent };
  const matching = rules.filter((rule) => Object.entries(rule.attrs).every(([k, v]) => current[k] === v));
  matching.sort((a, b) => Object.keys(a.attrs).length - Object.keys(b.attrs).length || a.order - b.order);
  const vars: Record<string, string> = {};
  for (const rule of matching) Object.assign(vars, rule.decls);
  return vars;
}

function mix(a: Rgba, pa: number, b: Rgba): Rgba {
  // color-mix(in srgb): premultiplied interpolation, result alpha = weighted alpha.
  const pb = 1 - pa;
  const alpha = a.a * pa + b.a * pb;
  if (alpha === 0) return { r: 0, g: 0, b: 0, a: 0 };
  return {
    r: (a.r * a.a * pa + b.r * b.a * pb) / alpha,
    g: (a.g * a.a * pa + b.g * b.a * pb) / alpha,
    b: (a.b * a.a * pa + b.b * b.a * pb) / alpha,
    a: alpha,
  };
}

/** Resolves a custom property to a colour: hex, rgb(a), var(), and `color-mix(in srgb, X p%, Y)`. */
export function resolveColor(name: string, vars: Record<string, string>, depth = 0): Rgba {
  if (depth > 12) throw new Error(`Cyclic token ${name}`);
  const raw = vars[name];
  if (raw === undefined) throw new Error(`Token ${name} is not defined`);
  return resolveValue(raw, vars, depth + 1);
}

function splitTop(input: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < input.length; i++) {
    if (input[i] === "(") depth++;
    else if (input[i] === ")") depth--;
    else if (input[i] === "," && depth === 0) {
      parts.push(input.slice(start, i).trim());
      start = i + 1;
    }
  }
  parts.push(input.slice(start).trim());
  return parts;
}

function resolveValue(raw: string, vars: Record<string, string>, depth: number): Rgba {
  const value = raw.trim();
  const ref = /^var\((--[a-z0-9-]+)\)$/.exec(value);
  if (ref) return resolveColor(ref[1] ?? "", vars, depth);
  const m = /^color-mix\((.*)\)$/.exec(value);
  if (m) {
    const [space = "", first = "", second = ""] = splitTop(m[1] ?? "");
    if (space !== "in srgb") throw new Error(`Unsupported colour space: ${space}`);
    const firstMatch = /^(.*?)\s+([\d.]+)%$/.exec(first);
    if (!firstMatch) throw new Error(`Unsupported color-mix: ${value}`);
    return mix(resolveValue(firstMatch[1] ?? "", vars, depth + 1), Number.parseFloat(firstMatch[2] ?? "0") / 100, resolveValue(second, vars, depth + 1));
  }
  return parseColor(value);
}
