import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  it("keeps the color and the font size when both come from the design tokens", () => {
    expect(cn("text-primary-foreground", "text-body")).toBe("text-primary-foreground text-body");
    expect(cn("text-small", "text-content")).toBe("text-small text-content");
  });
  it("still lets a later size or color win", () => {
    expect(cn("text-small", "text-body")).toBe("text-body");
    expect(cn("text-content", "text-muted-text")).toBe("text-muted-text");
    expect(cn("rounded-row", "rounded-pill")).toBe("rounded-pill");
  });
});
