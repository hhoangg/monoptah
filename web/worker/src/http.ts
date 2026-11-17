import type { Context } from "hono";
import type { ContentfulStatusCode } from "hono/utils/http-status";
import { type ApiError, type ApiErrorCode } from "@monoptah/protocol";
import type { z } from "zod";
import type { Env } from "./env.ts";

/** Hono environment of every route. `Variables` is where the auth resolver (A-04) puts the principal. */
export interface AppEnv {
  Bindings: Env;
  Variables: Record<string, never>;
}

export type AppContext = Context<AppEnv>;

/** Error body `{error, code}` (security.md 3 and architecture.md 4.1). */
export function apiError(c: AppContext, status: ContentfulStatusCode, code: ApiErrorCode, message: string): Response {
  const body: ApiError = { error: message, code };
  return c.json(body, status);
}

/**
 * Success body typed against a protocol response schema: a handler passing data that does not match
 * the route's schema fails to compile. The data is not parsed at run time.
 */
export function respond<S extends z.ZodType>(c: AppContext, _schema: S, data: z.output<S>, status: ContentfulStatusCode = 200): Response {
  return c.json(data as never, status);
}
