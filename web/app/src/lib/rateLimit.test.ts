import type { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/features/accounts/api";
import { createQueryClient } from "./queryClient";
import {
  defaultRetry,
  MAX_RATE_LIMIT_RETRIES,
  MAX_RATE_LIMIT_WAIT_MS,
  queryRetryDelay,
  rateLimitWaitMs,
  RATE_LIMIT_JITTER_MS,
  withRateLimitRetry,
} from "./rateLimit";

function limited(retryAfter: number | null) {
  return new ApiError({ status: 429, code: "rate_limited", message: "Too many requests", retryAfter });
}
function failed(status: number, code: ApiError["code"]) {
  return new ApiError({ status, code, message: "x" });
}

describe("rateLimitWaitMs", () => {
  it("waits for Retry-After plus a jitter below 500 ms", () => {
    expect(rateLimitWaitMs(limited(3), 0, () => 0)).toBe(3000);
    expect(rateLimitWaitMs(limited(3), 0, () => 0.999)).toBeLessThan(3000 + RATE_LIMIT_JITTER_MS);
  });

  it("falls back to exponential backoff without a header", () => {
    expect(rateLimitWaitMs(limited(null), 0, () => 0)).toBe(2000);
    expect(rateLimitWaitMs(limited(null), 1, () => 0)).toBe(4000);
    expect(rateLimitWaitMs(limited(null), 2, () => 0)).toBe(8000);
  });

  it("caps the wait at 60 s", () => {
    expect(rateLimitWaitMs(limited(900), 0, () => 0)).toBe(MAX_RATE_LIMIT_WAIT_MS);
    expect(rateLimitWaitMs(limited(null), 10, () => 0)).toBe(MAX_RATE_LIMIT_WAIT_MS);
  });
});

describe("retry rules", () => {
  it("retries a 429 a few times, then gives up", () => {
    expect(defaultRetry(0, limited(1))).toBe(true);
    expect(defaultRetry(MAX_RATE_LIMIT_RETRIES - 1, limited(1))).toBe(true);
    expect(defaultRetry(MAX_RATE_LIMIT_RETRIES, limited(1))).toBe(false);
  });

  it("keeps one retry for other errors", () => {
    expect(defaultRetry(0, failed(500, "internal"))).toBe(true);
    expect(defaultRetry(1, failed(500, "internal"))).toBe(false);
  });

  it("lets a custom rule decline 401, 403 and not_found but still retries a 429", () => {
    const rule = withRateLimitRetry((count, error) => !(error instanceof ApiError && error.status < 500) && count < 1);
    expect(rule(0, failed(401, "unauthorized"))).toBe(false);
    expect(rule(0, failed(403, "forbidden"))).toBe(false);
    expect(rule(0, failed(404, "not_found"))).toBe(false);
    expect(rule(0, limited(1))).toBe(true);
  });

  it("delays by Retry-After for a 429 and by the TanStack default for the rest", () => {
    expect(queryRetryDelay(0, limited(7))).toBeGreaterThanOrEqual(7000);
    expect(queryRetryDelay(0, failed(500, "internal"))).toBe(1000);
    expect(queryRetryDelay(3, failed(500, "internal"))).toBe(8000);
  });
});

describe("query client", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("does not retry mutations", () => {
    expect(createQueryClient().getDefaultOptions().mutations?.retry).toBe(0);
  });

  function run(client: QueryClient, errors: ApiError[]) {
    let calls = 0;
    const promise = client
      .fetchQuery({
        queryKey: ["probe"],
        queryFn: async () => {
          const error = errors[calls++];
          if (error) throw error;
          return "ok";
        },
      })
      .then(
        (value) => ({ value }),
        (error: unknown) => ({ error }),
      );
    return { promise, calls: () => calls };
  }

  it("fetchQuery waits for Retry-After on a 429 and then succeeds (the defaults apply to imperative fetches)", async () => {
    const { promise, calls } = run(createQueryClient(), [limited(2)]);
    await vi.advanceTimersByTimeAsync(1900);
    expect(calls()).toBe(1);
    await vi.advanceTimersByTimeAsync(700);
    expect(await promise).toEqual({ value: "ok" });
    expect(calls()).toBe(2);
  });

  it("gives up after the retry cap and rejects with the 429", async () => {
    const errors = Array.from({ length: 10 }, () => limited(1));
    const { promise, calls } = run(createQueryClient(), errors);
    await vi.advanceTimersByTimeAsync(10_000);
    const result = await promise;
    expect("error" in result && (result.error as ApiError).status).toBe(429);
    expect(calls()).toBe(MAX_RATE_LIMIT_RETRIES + 1);
  });

  it("gives other errors one retry only", async () => {
    const error = failed(500, "internal");
    const { promise, calls } = run(createQueryClient(), [error, error, error]);
    await vi.advanceTimersByTimeAsync(5000);
    await promise;
    expect(calls()).toBe(2);
  });
});
