// The Monoptah web Worker: one Hono app (architecture.md 4.1) plus the DesktopRoom Durable Object.
import { Hono } from "hono";
import type { Env } from "./env.ts";
import { apiError, type AppEnv } from "./http.ts";
import { scheduled } from "./cron.ts";
import { securityHeaders } from "./middleware/security.ts";
import { MODULES } from "./modules/index.ts";

export { DesktopRoom } from "./relay/DesktopRoom.ts";

export function createApp(): Hono<AppEnv> {
  const app = new Hono<AppEnv>();

  // 1. Security headers on every response. Per-route checks (Origin, CSRF, rate limit, auth) run
  //    inside each module route, in the order of architecture.md 4.1 (see modules/define.ts).
  app.use("*", securityHeaders);

  // 2. One sub-app per module, generated from the protocol route registry.
  for (const entry of Object.values(MODULES)) {
    if ("routes" in entry) app.route("/", entry.routes);
  }

  // 3. Unknown API and relay paths are JSON 404s; everything else is the SPA.
  const notFound = (c: Parameters<typeof apiError>[0]) => apiError(c, 404, "not_found", "Not found");
  app.all("/api/*", notFound);
  app.all("/relay/*", notFound);
  app.all("*", (c) => c.env.ASSETS.fetch(c.req.raw));

  app.onError((err, c) => {
    // Metadata only: no message (it may echo input), no stack.
    console.error(`unhandled error: route=${c.req.method} ${new URL(c.req.url).pathname} error=${err.name}`);
    return apiError(c, 500, "internal", "Internal error");
  });
  return app;
}

const app = createApp();

export default {
  fetch: app.fetch,
  scheduled,
} satisfies ExportedHandler<Env>;

