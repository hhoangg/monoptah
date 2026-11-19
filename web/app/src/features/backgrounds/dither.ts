// Rewritten from mobile/src/features/background/dither.ts (itself from the desktop's
// newThreadBackgroundEffects.worker.ts), so every client draws the same artwork. Pure functions over RGBA
// bytes; the worker and the main-thread fallback both call them.

/** An image as RGBA, `width` x `height`. */
export interface Source {
  width: number;
  height: number;
  pixels: Uint8ClampedArray;
}

/** The long edge of the stored picture; larger photos are scaled down first. */
export const MAX_EDGE = 1024;

/** 0.05 to 0.65, the desktop's range. */
export const OPACITY_RANGE = { min: 0.05, max: 0.65 } as const;
export const DEFAULT_OPACITY = 0.45;

export function clampOpacity(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_OPACITY;
  return Math.min(OPACITY_RANGE.max, Math.max(OPACITY_RANGE.min, value));
}

/** The size that fits `width` x `height` into MAX_EDGE, keeping the aspect ratio. */
export function fitSize(width: number, height: number): { width: number; height: number } {
  const scale = Math.min(1, MAX_EDGE / Math.max(width, height, 1));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

const BAYER = [
  [0, 8, 2, 10],
  [12, 4, 14, 6],
  [3, 11, 1, 9],
  [15, 7, 13, 5],
];

/** The image rebuilt with a Bayer-dithered palette, in 2x2 blocks, as RGBA. */
export function ditherPixels(source: Source): Uint8ClampedArray {
  const { width, height, pixels } = source;
  const output = new Uint8ClampedArray(pixels.length);
  for (let y = 0; y < height; y += 2) {
    for (let x = 0; x < width; x += 2) {
      // The block takes the colour of its bottom-right pixel (clamped to the edge).
      const base = (Math.min(y + 1, height - 1) * width + Math.min(x + 1, width - 1)) * 4;
      const r = pixels[base] ?? 0;
      const g = pixels[base + 1] ?? 0;
      const b = pixels[base + 2] ?? 0;
      const a = pixels[base + 3] ?? 255;
      const peak = Math.max(r, g, b);
      const threshold = BAYER[Math.floor(y / 2) % 4]?.[Math.floor(x / 2) % 4] ?? 0;
      const gain = peak / 255 > (threshold + 0.5) / 16 ? 255 / Math.max(peak, 1) : 0.08;
      const cr = Math.min(255, Math.round(r * gain));
      const cg = Math.min(255, Math.round(g * gain));
      const cb = Math.min(255, Math.round(b * gain));
      for (let dy = 0; dy < Math.min(2, height - y); dy += 1) {
        for (let dx = 0; dx < Math.min(2, width - x); dx += 1) {
          const at = ((y + dy) * width + x + dx) * 4;
          output[at] = cr;
          output[at + 1] = cg;
          output[at + 2] = cb;
          output[at + 3] = a;
        }
      }
    }
  }
  return output;
}

// The fade of the desktop's "Haze" chat background: the image darkens toward the bottom under an overlay of the
// background colour and is gone by 90% of the height. Stops copied from the mobile port.
const MASK = {
  values: [1, 1, 0.94, 0.72, 0.42, 0.16, 0.04, 0],
  locations: [0, 0.19, 0.3, 0.42, 0.54, 0.66, 0.78, 0.9],
};
const OVERLAY = {
  values: [0, 0.26, 0.55, 0.78],
  locations: [0.16, 0.38, 0.55, 0.75],
};

/** A gradient's value at `t`, held at its first and last stops outside them. */
function gradientAt({ values, locations }: { values: number[]; locations: number[] }, t: number): number {
  const first = locations[0] ?? 0;
  if (t <= first) return values[0] ?? 0;
  for (let i = 1; i < locations.length; i += 1) {
    const to = locations[i] ?? 1;
    if (t <= to) {
      const from = locations[i - 1] ?? 0;
      const span = (t - from) / (to - from);
      const a = values[i - 1] ?? 0;
      return a + ((values[i] ?? a) - a) * span;
    }
  }
  return values[values.length - 1] ?? 0;
}

/**
 * The image with its fade baked into its alpha, so the page colour shows through as the mask and overlay would
 * let it, in every theme, without a gradient layer at runtime.
 */
export function fadeOut(source: Source): Uint8ClampedArray {
  const { width, height, pixels } = source;
  const output = new Uint8ClampedArray(pixels);
  for (let y = 0; y < height; y += 1) {
    const t = (y + 0.5) / height;
    const alpha = Math.round(255 * (1 - gradientAt(OVERLAY, t)) * gradientAt(MASK, t));
    for (let x = 0; x < width; x += 1) {
      const at = (y * width + x) * 4 + 3;
      output[at] = Math.round(((pixels[at] ?? 255) * alpha) / 255);
    }
  }
  return output;
}

/** Dither, then fade: the pixels that get stored. */
export function renderPixels(source: Source): Uint8ClampedArray {
  const dithered = ditherPixels(source);
  return fadeOut({ width: source.width, height: source.height, pixels: dithered });
}
