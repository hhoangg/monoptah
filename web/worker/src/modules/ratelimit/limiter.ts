// D1 fixed-window limiter (security.md 3.5). One D1 statement per call.
import type { Env } from "../../env.ts";
import { nowSeconds } from "../../lib/time.ts";

export interface ConsumeResult {
  /** True while the call is within the limit. */
  ok: boolean;
  /** Seconds until the window ends, at least 1. Meaningful when `ok` is false; the `Retry-After` value. */
  retryAfter: number;
  /** The counter value after this call (1 for the first call of a window). */
  count: number;
  /** True for exactly one call per key and window: the call that first exceeded the limit (audit it once). */
  firstBreach: boolean;
}

/** Statement of the limiter, exported for the query plan test. */
export const CONSUME_SQL =
  "INSERT INTO rate_limits (key, window_start, count) VALUES (?, ?, 1) ON CONFLICT (key, window_start) DO UPDATE SET count = count + 1 RETURNING count";

/**
 * Counts one call against `key` in the fixed window of `windowSeconds` containing `now` and reports whether
 * it is within `limit`. The upsert and the read are one atomic D1 statement (`RETURNING count`), so concurrent
 * calls cannot undercount. A refused call still increments the counter (bounded by the window and by the daily
 * cleanup of `rate_limits`).
 *
 * `key` is a namespaced string and must never contain a raw IP, invite code, handle, token or device code:
 * build it with `rateLimitKey` from salted or SHA-256 hashes. Windows are aligned to multiples of
 * `windowSeconds` since the epoch, so a burst across a boundary can reach twice the limit (fixed window).
 */
export async function consume(
  env: Pick<Env, "DB">,
  key: string,
  limit: number,
  windowSeconds: number,
  now: number = nowSeconds(),
): Promise<ConsumeResult> {
  const windowStart = now - (now % windowSeconds);
  const row = await env.DB.prepare(CONSUME_SQL).bind(key, windowStart).first<{ count: number }>();
  const count = row?.count ?? 1;
  return {
    ok: count <= limit,
    retryAfter: Math.max(1, windowStart + windowSeconds - now),
    count,
    firstBreach: count === limit + 1,
  };
}

/** Joins the parts of a key: `group:kind:hashedIdentifier`. Parts must already be hashes or fixed words. */
export function rateLimitKey(...parts: string[]): string {
  return parts.join(":");
}
