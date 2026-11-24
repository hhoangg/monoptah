import { describe, expect, it } from "vitest";
import { canCreateMono } from "./workspaceFilter";

describe("canCreateMono", () => {
  it("is true when the catalog carries monoTools (the real desktop), whatever the capabilities", () => {
    expect(canCreateMono(true, false)).toBe(true);
    expect(canCreateMono(true)).toBe(true);
  });

  it("is true for an older desktop that only advertised the monoTools capability", () => {
    expect(canCreateMono(undefined, true)).toBe(true);
    expect(canCreateMono(false, true)).toBe(true);
  });

  it("is false when neither the catalog nor the capabilities offer Mono tools", () => {
    expect(canCreateMono(undefined, false)).toBe(false);
    expect(canCreateMono(false, false)).toBe(false);
    expect(canCreateMono(undefined)).toBe(false);
  });
});
