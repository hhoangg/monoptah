import { Hono, type Handler } from "hono";
import { routesOfModule, type ApiModule, type ApiRoute } from "@monoptah/protocol";
import { apiError, type AppEnv } from "../http.ts";
import { authResolver } from "../middleware/auth.ts";
import { bodySize } from "../middleware/body-size.ts";
import { browserChecks } from "../middleware/browser.ts";
import { rateLimit } from "./ratelimit/index.ts";

/** Key of a route in a module's handler map, for example `GET /api/me`. */
export function routeKey(route: Pick<ApiRoute, "method" | "path">): string {
  return `${route.method} ${route.path}`;
}

/** The 501 every route answers until its ticket fills it in (workstreams.md 2.1, contract e). */
export const unsupported: Handler<AppEnv> = (c) => apiError(c, 501, "unsupported", "Not implemented yet");

/**
 * Builds the Hono sub-app of one module from the `@monoptah/protocol` route registry, so every route of
 * architecture.md 4.2 exists. Each route runs, in order: the body size limit (32 KiB), the browser checks (Origin and CSRF for
 * cookie routes, cookie stripping for token and pairing routes), the rate limit hook, the auth resolver,
 * then its handler. A workstream implements a route by passing it in `handlers` under `routeKey(route)`;
 * routes without a handler answer 501 `{error, code: "unsupported"}`.
 */
export function defineModule(module: ApiModule, handlers: Record<string, Handler<AppEnv>> = {}): Hono<AppEnv> {
  const routes = routesOfModule(module);
  const known = new Set(routes.map(routeKey));
  for (const key of Object.keys(handlers)) {
    if (!known.has(key)) throw new Error(`Module ${module} has no route "${key}" in the protocol registry`);
  }
  const app = new Hono<AppEnv>();
  for (const route of routes) {
    app.on(route.method, route.path, bodySize, browserChecks(route), rateLimit(route), authResolver(route), handlers[routeKey(route)] ?? unsupported);
  }
  return app;
}
