import { describe, expect, it } from "vitest";
import { z } from "zod";
import { ApiErrorSchema } from "../src/index.ts";

// W13-8: zod's JIT compiler probes `new Function("")`, which raises a CSP / Trusted Types violation report.
describe("zod config", () => {
  it("runs without the JIT compiler once the protocol package is loaded", () => {
    expect(z.config().jitless).toBe(true);
  });

  it("still validates objects", () => {
    expect(ApiErrorSchema.safeParse({ error: "x", code: "internal" }).success).toBe(true);
    expect(ApiErrorSchema.safeParse({ error: 1 }).success).toBe(false);
  });
});
