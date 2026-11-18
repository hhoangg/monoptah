import { describe, expect, it } from "vitest";
import { trimSlashes } from "./trimSlashes";

describe("trimSlashes", () => {
  it("drops trailing slashes only", () => {
    expect(trimSlashes("/a/b/")).toBe("/a/b");
    expect(trimSlashes("/a//b///")).toBe("/a//b");
    expect(trimSlashes("/")).toBe("");
    expect(trimSlashes("")).toBe("");
    expect(trimSlashes("/a/b")).toBe("/a/b");
  });

  it("stays linear on a long run of slashes", () => {
    const started = performance.now();
    expect(trimSlashes("/".repeat(200_000) + "x")).toBe("/".repeat(200_000) + "x");
    expect(trimSlashes("x" + "/".repeat(200_000))).toBe("x");
    expect(performance.now() - started).toBeLessThan(200);
  });
});
