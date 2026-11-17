import { HealthResponse } from "@monoptah/protocol";
import { respond } from "../../http.ts";
import { defineModule } from "../define.ts";

/** `GET /api/health`: liveness probe, no D1 read. The only route that is not a 501 stub. */
export const routes = defineModule("health", {
  "GET /api/health": (c) => respond(c, HealthResponse, { ok: true }),
});
