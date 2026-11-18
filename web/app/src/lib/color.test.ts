// Copied from src/shared/lib/colorUtils.test.ts at ca73ac2b
import { describe, expect, it } from "vitest";
import { hexToHsv, hsvToHex, hslToRgb, isHexColor, safeHexColor } from "./color";

describe("hslToRgb", () => {
  it("matches the theme backgrounds the native window is filled with", () => {
    expect(hslToRgb(240, 0, 9)).toEqual({ r: 23, g: 23, b: 23 });
    expect(hslToRgb(240, 0, 97)).toEqual({ r: 247, g: 247, b: 247 });
  });

  it("tints toward the accent hue", () => {
    expect(hslToRgb(0, 100, 50)).toEqual({ r: 255, g: 0, b: 0 });
    expect(hslToRgb(120, 100, 50)).toEqual({ r: 0, g: 255, b: 0 });
    expect(hslToRgb(240, 100, 50)).toEqual({ r: 0, g: 0, b: 255 });
  });

  it("wraps hue and clamps the ends", () => {
    expect(hslToRgb(-120, 100, 50)).toEqual(hslToRgb(240, 100, 50));
    expect(hslToRgb(600, 100, 50)).toEqual(hslToRgb(240, 100, 50));
    expect(hslToRgb(240, 0, -20)).toEqual({ r: 0, g: 0, b: 0 });
    expect(hslToRgb(240, 0, 120)).toEqual({ r: 255, g: 255, b: 255 });
  });
});

describe("hex validation", () => {
  it("accepts only strict six-digit hex", () => {
    expect(isHexColor("#a1B2c3")).toBe(true);
    for (const bad of ["a1b2c3", "#abc", "#a1b2c3;x", "red", "#a1b2c3 ", "url(x)", ""]) {
      expect(isHexColor(bad)).toBe(false);
    }
  });

  it("safeHexColor never lets a non-hex value through", () => {
    expect(safeHexColor("#A1B2C3")).toBe("#a1b2c3");
    expect(safeHexColor("red; background: url(//evil)")).toBe("#808080");
    expect(safeHexColor(42, "#000000")).toBe("#000000");
    expect(safeHexColor(undefined)).toBe("#808080");
  });

  it("round-trips hsv", () => {
    expect(hsvToHex(0, 100, 100)).toBe("#ff0000");
    const { h, s, v } = hexToHsv("#00ff00");
    expect(hsvToHex(h, s, v)).toBe("#00ff00");
  });
});
