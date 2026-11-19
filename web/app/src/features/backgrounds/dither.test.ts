import { describe, expect, it } from "vitest";
import { clampOpacity, DEFAULT_OPACITY, ditherPixels, fadeOut, fitSize, MAX_EDGE, OPACITY_RANGE, renderPixels } from "./dither";

function solid(width: number, height: number, rgba: [number, number, number, number]) {
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < width * height; i += 1) pixels.set(rgba, i * 4);
  return { width, height, pixels };
}

describe("dither", () => {
  it("clamps opacity to the desktop range", () => {
    expect(clampOpacity(0)).toBe(OPACITY_RANGE.min);
    expect(clampOpacity(1)).toBe(OPACITY_RANGE.max);
    expect(clampOpacity(0.3)).toBe(0.3);
    expect(clampOpacity(Number.NaN)).toBe(DEFAULT_OPACITY);
  });

  it("scales large photos down to the max edge and keeps small ones", () => {
    expect(fitSize(4000, 2000)).toEqual({ width: MAX_EDGE, height: MAX_EDGE / 2 });
    expect(fitSize(300, 200)).toEqual({ width: 300, height: 200 });
  });

  it("dithers in 2x2 blocks of one colour and keeps alpha", () => {
    const out = ditherPixels(solid(4, 4, [200, 100, 50, 128]));
    for (let by = 0; by < 4; by += 2) {
      for (let bx = 0; bx < 4; bx += 2) {
        const first = (by * 4 + bx) * 4;
        for (const [dx, dy] of [[1, 0], [0, 1], [1, 1]] as const) {
          const at = ((by + dy) * 4 + bx + dx) * 4;
          expect([...out.slice(at, at + 4)]).toEqual([...out.slice(first, first + 4)]);
        }
        expect(out[first + 3]).toBe(128);
      }
    }
  });

  it("lights a bright pixel at full gain and dims a black one", () => {
    const bright = ditherPixels(solid(2, 2, [255, 255, 255, 255]));
    expect([...bright.slice(0, 3)]).toEqual([255, 255, 255]);
    const black = ditherPixels(solid(2, 2, [0, 0, 0, 255]));
    expect([...black.slice(0, 3)]).toEqual([0, 0, 0]);
  });

  it("fades alpha to nothing toward the bottom and leaves colour alone", () => {
    const height = 100;
    const out = fadeOut(solid(1, height, [10, 20, 30, 255]));
    expect(out[3]).toBe(255);
    expect(out[(height - 1) * 4 + 3]).toBe(0);
    expect([...out.slice(0, 3)]).toEqual([10, 20, 30]);
    const alphas = Array.from({ length: height }, (_, y) => out[y * 4 + 3] ?? 0);
    expect(alphas).toEqual([...alphas].sort((a, b) => b - a));
  });

  it("renderPixels returns the same size", () => {
    const source = solid(5, 3, [90, 90, 90, 255]);
    expect(renderPixels(source)).toHaveLength(source.pixels.length);
  });
});
