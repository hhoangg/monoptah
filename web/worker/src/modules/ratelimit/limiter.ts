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
  "INSERT INTO rate_limits (key, window_start, count) VALUES (?, ?, 1) ON CONFLICT (key, window_start) DO UPDATE SET count = count + 1 WHERE count <= ? RETURNING count";

/**
 * Counts one call against `key` in the fixed window of `windowSeconds` containing `now` and reports whether
 * it is within `limit`. The upsert and the read are one atomic D1 statement (`RETURNING count`), so concurrent
 * calls cannot undercount. The update is conditional on `count <= limit`, so the first refused call of a window
 * still writes once (count = limit + 1, the call that is audited) and every later refused call writes nothing:
 * no row comes back and the key is still limited. A flood therefore costs at most one D1 write per key and
 * window instead of one per request (the free plan has 100,000 writes per day).
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
  const retryAfter = Math.max(1, windowStart + windowSeconds - now);
  const row = await env.DB.prepare(CONSUME_SQL).bind(key, windowStart, limit).first<{ count: number }>();
  // No row: the conflict update was skipped because the counter is already past the limit.
  if (row === null) return { ok: false, retryAfter, count: limit + 1, firstBreach: false };
  return { ok: row.count <= limit, retryAfter, count: row.count, firstBreach: row.count === limit + 1 };
}

/** Joins the parts of a key: `group:kind:hashedIdentifier`. Parts must already be hashes or fixed words. */
export function rateLimitKey(...parts: string[]): string {
  return parts.join(":");
}
