import { describe, expect, it } from "vitest";
import { API_MODULES, API_ROUTES, ApiErrorSchema, HealthResponse } from "@monoptah/protocol";
import { MODULES } from "../src/modules/index.ts";
import { BROWSER_HEADERS, call } from "./helpers.ts";

/** `/api/desktops/:id/grants/:deviceId` -> `/api/desktops/x/grants/y`. */
function concretePath(path: string): string {
  return path.replace(/:[A-Za-z]+/g, "x") + (path.startsWith("/relay/device/") ? "?device=y" : "");
}

describe("module mounting", () => {
  it("has a module for every module of the protocol registry, plus ratelimit", () => {
    const names = new Set(Object.keys(MODULES));
    for (const module of API_MODULES) expect(names.has(module), module).toBe(true);
    expect(names.has("ratelimit")).toBe(true);
    // The fourteen modules of workstreams.md 3.1 are all there.
    for (const module of ["auth", "sessions", "recovery", "invites", "passkeys", "admin", "ratelimit", "audit", "relay", "pairing", "desktops", "devices", "grants", "push"]) {
      expect(names.has(module), module).toBe(true);
    }
  });
});

describe("GET /api/health", () => {
  it("answers {ok: true} without touching D1", async () => {
    const res = await call("/api/health");
    expect(res.status).toBe(200);
    const body: unknown = await res.json();
    expect(HealthResponse.parse(body)).toEqual({ ok: true });
  });
});

describe("every registry route", () => {
  const stubs = API_ROUTES.filter((route) => !(route.method === "GET" && route.path === "/api/health"));

  it("covers the whole registry", () => {
    expect(stubs.length).toBe(API_ROUTES.length - 1);
    expect(stubs.length).toBeGreaterThan(50);
  });

  it.each(stubs.map((route) => [`${route.method} ${route.path}`, route] as const))("%s answers 501 unsupported", async (_name, route) => {
    const hasBody = route.method !== "GET" && route.method !== "DELETE";
    const res = await call(concretePath(route.path), {
      method: route.method,
      headers: { ...BROWSER_HEADERS, ...(hasBody ? { "Content-Type": "application/json" } : {}) },
      body: hasBody ? "{}" : undefined,
    });
    expect(res.status).toBe(501);
    const parsed = ApiErrorSchema.parse(await res.json());
    expect(parsed.code).toBe("unsupported");
  });
});

describe("unknown paths", () => {
  it("answers 404 not_found for an unknown /api path", async () => {
    const res = await call("/api/nope");
    expect(res.status).toBe(404);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("not_found");
  });

  it("answers 404 not_found for a method the route does not have", async () => {
    const res = await call("/api/health", { method: "POST", headers: BROWSER_HEADERS });
    expect(res.status).toBe(404);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("not_found");
  });

  it("answers 404 not_found for an unknown /relay path", async () => {
    const res = await call("/relay/nope");
    expect(res.status).toBe(404);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("not_found");
  });
});
