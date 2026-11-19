import { describe, expect, it } from "vitest";
import { DESKTOP_COLORS, FALLBACK_COLOR, findDesktopColor, resolveDesktopColor } from "./colors";

describe("desktop colors", () => {
  it("has the 9 swatches of the design system", () => {
    expect(DESKTOP_COLORS.map((color) => color.value)).toEqual(["#459cf7", "#ea603e", "#f4bf25", "#39c363", "#e45a9f", "#b168d4", "#37beb4", "#ef8c39", "#8b63e9"]);
  });

  it("uses the dark muted token value (design-system.md section 1) as the fallback", () => {
    expect(FALLBACK_COLOR.value).toBe("#8c8c8c");
  });

  it("matches a stored palette value case-insensitively", () => {
    expect(resolveDesktopColor("#EA603E").name).toBe("Red");
  });

  it("falls back for unknown, empty and hostile strings, never passing them through", () => {
    for (const stored of [null, undefined, "", "red", "url(javascript:alert(1))", "#fff; background: url(x)"]) {
      expect(findDesktopColor(stored)).toBeNull();
      expect(resolveDesktopColor(stored)).toBe(FALLBACK_COLOR);
    }
  });
});
