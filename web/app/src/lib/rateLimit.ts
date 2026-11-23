// One retry policy for the Worker's 429 (`rate_limited`, `Retry-After` in seconds). Queries are idempotent GETs, so they
// wait the time the server asked for and try again; mutations never retry (`mutations: { retry: 0 }` in queryClient.ts).
// Queries that override `retry` wrap their own rule in `withRateLimitRetry`. A few use `retry: false` on purpose (the
// sign-in, recovery and pairing screens, the shell, push onboarding): they show the 429 inline instead of waiting.
import { ApiError } from "@/features/accounts/api";

/** The Worker's window is 60 s, so waiting longer than that never helps. */
export const MAX_RATE_LIMIT_WAIT_MS = 60_000;
/** Retries after the first 429 (so up to 4 calls). */
export const MAX_RATE_LIMIT_RETRIES = 3;
/** Spread of the wait so tabs and queries that were limited together do not all retry in the same instant. */
export const RATE_LIMIT_JITTER_MS = 500;

export type RetryRule = (failureCount: number, error: Error) => boolean;

export function isRateLimited(error: unknown): error is ApiError {
  return error instanceof ApiError && (error.status === 429 || error.code === "rate_limited");
}

/** How long to wait before retry number `failureCount + 1`: `Retry-After` if known, else exponential from 2 s; capped. */
export function rateLimitWaitMs(error: ApiError, failureCount: number, random: () => number = Math.random): number {
  const base = error.retryAfter !== null ? error.retryAfter * 1000 : 2000 * 2 ** failureCount;
  return Math.min(MAX_RATE_LIMIT_WAIT_MS, base) + Math.floor(random() * RATE_LIMIT_JITTER_MS);
}

/** Wraps a query's own `retry` rule: 429 is retried a few times, every other error follows `base`. */
export function withRateLimitRetry(base: RetryRule): RetryRule {
  return (failureCount, error) => (isRateLimited(error) ? failureCount < MAX_RATE_LIMIT_RETRIES : base(failureCount, error));
}

/** Default rule: one retry for anything else, a few for 429. */
export const defaultRetry: RetryRule = withRateLimitRetry((failureCount) => failureCount < 1);

/** TanStack's default delay (1 s doubling, up to 30 s) for everything except 429. */
export function queryRetryDelay(failureCount: number, error: Error): number {
  if (isRateLimited(error)) return rateLimitWaitMs(error, failureCount);
  return Math.min(1000 * 2 ** failureCount, 30_000);
}
