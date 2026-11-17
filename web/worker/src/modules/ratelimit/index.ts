import type { MiddlewareHandler } from "hono";
import type { ApiRoute } from "@monoptah/protocol";
import type { AppEnv } from "../../http.ts";

/**
 * Rate limit hook, one per route (architecture.md 4.1). STUB: A-07 implements the table of
 * security.md 3.5 (D1 `rate_limits` fixed windows for auth, pairing and notify; an in-memory
 * per-isolate counter for the general 120 per session per minute) and answers 429 with `Retry-After`.
 * `route.rateLimited` marks the public routes that need a per-IP window. This module has no routes of its own.
 */
export function rateLimit(_route: ApiRoute): MiddlewareHandler<AppEnv> {
  return async (_c, next) => {
    await next();
  };
}
