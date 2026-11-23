import { afterEach, describe, expect, it } from "vitest";
import { clearRateLimited, FALLBACK_RETRY_AFTER_SECONDS, noteRateLimited, rateLimitSecondsLeft, useRateLimitState } from "./rateLimitState";

afterEach(() => clearRateLimited());

describe("rate limit state", () => {
  it("starts clear", () => {
    expect(useRateLimitState.getState().until).toBeNull();
  });

  it("sets the end of the wait from Retry-After", () => {
    noteRateLimited(10, 1_000);
    expect(useRateLimitState.getState().until).toBe(11_000);
  });

  it("uses a fallback without a header and never shortens a longer wait", () => {
    noteRateLimited(30, 0);
    noteRateLimited(null, 0);
    expect(useRateLimitState.getState().until).toBe(30_000);
    clearRateLimited();
    noteRateLimited(null, 0);
    expect(useRateLimitState.getState().until).toBe(FALLBACK_RETRY_AFTER_SECONDS * 1000);
  });

  it("clears", () => {
    noteRateLimited(5);
    clearRateLimited();
    expect(useRateLimitState.getState().until).toBeNull();
  });

  it("counts whole seconds left and reaches 0 when the window passes", () => {
    expect(rateLimitSecondsLeft(null, 0)).toBe(0);
    expect(rateLimitSecondsLeft(2_500, 0)).toBe(3);
    expect(rateLimitSecondsLeft(2_500, 2_400)).toBe(1);
    expect(rateLimitSecondsLeft(2_500, 2_500)).toBe(0);
  });
});
