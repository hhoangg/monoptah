import type { MiddlewareHandler } from "hono";
import type { ApiRoute } from "@monoptah/protocol";
import type { AppEnv } from "../http.ts";

/**
 * Auth resolver, one per route (architecture.md 4.1). STUB: A-03/A-04 resolve the session cookie
 * (`session`, `fresh`, `admin`, `session+grant`) and A-04/R-03 the desktop bearer token (`token:*`),
 * set the principal on the context and answer 401/403. Today it lets every request through to the
 * 501 handler. `route.auth` says what to resolve; it must only read the cookie for cookie routes.
 */
export function authResolver(_route: ApiRoute): MiddlewareHandler<AppEnv> {
  return async (_c, next) => {
    await next();
  };
}
