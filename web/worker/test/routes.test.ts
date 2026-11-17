import { env } from "cloudflare:workers";
import { beforeAll, describe, expect, it } from "vitest";
import { API_MODULES, API_ROUTES, ApiErrorSchema, HealthResponse } from "@monoptah/protocol";
import { MODULES } from "../src/modules/index.ts";
import { BROWSER_HEADERS, call } from "./helpers.ts";
import { createTestUser, loginAs, type TestSession } from "./support/accounts.ts";

/** Routes that have a handler now. Their behaviour is tested in their own files (sessions, me, audit tests). */
const IMPLEMENTED = new Set(["GET /api/me", "PATCH /api/me", "GET /api/audit", "GET /api/sessions", "DELETE /api/sessions/:id", "POST /api/auth/logout", "POST /api/auth/register/options", "POST /api/auth/register/verify", "POST /api/auth/login/options", "POST /api/auth/login/verify", "POST /api/auth/reauth/options", "POST /api/auth/reauth/verify", "GET /api/invites", "POST /api/invites", "DELETE /api/invites/:id"]);
const needsCookie = (route: { auth: string }) => ["session", "fresh", "admin", "session+grant"].includes(route.auth);

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
  const unimplemented = stubs.filter((route) => !IMPLEMENTED.has(`${route.method} ${route.path}`));
  let admin: TestSession;

  beforeAll(async () => {
    // An admin with a fresh session passes every session, fresh, admin and session+grant gate.
    admin = await loginAs((await createTestUser(env, { role: "admin" })).id, { fresh: true });
  });

  const send = (route: (typeof stubs)[number], cookie?: string) => {
    const hasBody = route.method !== "GET" && route.method !== "DELETE";
    return call(concretePath(route.path), {
      method: route.method,
      headers: { ...BROWSER_HEADERS, ...(cookie ? { Cookie: cookie } : {}) },
      body: hasBody ? "{}" : undefined,
    });
  };

  it("covers the whole registry", () => {
    expect(stubs.length).toBe(API_ROUTES.length - 1);
    expect(stubs.length).toBeGreaterThan(50);
    expect(unimplemented.length).toBe(stubs.length - IMPLEMENTED.size);
  });

  it.each(unimplemented.map((route) => [`${route.method} ${route.path}`, route] as const))("%s answers 501 unsupported with a valid session", async (_name, route) => {
    const res = await send(route, needsCookie(route) ? admin.cookie : undefined);
    expect(res.status).toBe(501);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("unsupported");
  });

  it.each(stubs.map((route) => [`${route.method} ${route.path}`, route] as const))("%s without a session: 401 on cookie routes, 501 on public and token routes", async (_name, route) => {
    const res = await send(route);
    if (needsCookie(route)) {
      expect(res.status).toBe(401);
      expect(ApiErrorSchema.parse(await res.json()).code).toBe("unauthorized");
    } else if (IMPLEMENTED.has(`${route.method} ${route.path}`)) {
      expect(res.status).not.toBe(501);
    } else {
      expect(res.status).toBe(501);
    }
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
