import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { API_MODULES, API_ROUTES, ApiErrorSchema, HealthResponse } from "@monoptah/protocol";
import { MODULES } from "../src/modules/index.ts";
import { BROWSER_HEADERS, call } from "./helpers.ts";
import { createTestUser, loginAs, type TestSession } from "./support/accounts.ts";

/**
 * Every route of the registry has a handler now (WS-A and WS-R) except account deletion (ticket U-11), which is
 * still the 501 stub. Their behaviour is tested in their own files.
 */
const STILL_STUBS = new Set(["DELETE /api/me"]);

/**
 * What a route without any credential answers: the handler's own validation or credential check, never a 501 stub.
 */
const ANONYMOUS_ANSWERS = new Map<string, { status: number; code?: string }>([
  ["POST /api/auth/register/options", { status: 400, code: "invalid" }],
  ["POST /api/auth/register/verify", { status: 400, code: "invalid" }],
  ["POST /api/auth/login/options", { status: 200 }],
  ["POST /api/auth/login/verify", { status: 400, code: "invalid" }],
  ["POST /api/auth/recover", { status: 400, code: "invalid" }],
  ["POST /api/pair/start", { status: 400, code: "invalid" }],
  ["POST /api/pair/poll", { status: 400, code: "invalid" }],
  ["GET /api/push/vapid-key", { status: 200 }],
  ["GET /api/desktop/me", { status: 401, code: "unauthorized" }],
  ["DELETE /api/desktop/me", { status: 401, code: "unauthorized" }],
  ["GET /api/desktop/grants", { status: 401, code: "unauthorized" }],
  ["POST /api/desktop/grants/:deviceId/state", { status: 401, code: "unauthorized" }],
  ["POST /api/desktop/grants/:deviceId/selection-ticket", { status: 401, code: "unauthorized" }],
  ["POST /api/push/notify", { status: 401, code: "unauthorized" }],
  ["GET /relay/desktop/:desktopId", { status: 426, code: "invalid" }],
]);
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

  // An admin with a fresh session passes every session, fresh, admin and session+grant gate. Each route gets its own,
  // because an earlier route (logout, session revoke) may end a shared one and turn the check into a vacuous 401.
  const freshAdmin = async (): Promise<TestSession> => loginAs((await createTestUser(env, { role: "admin" })).id, { fresh: true });

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
    for (const key of STILL_STUBS) expect(stubs.some((route) => `${route.method} ${route.path}` === key), key).toBe(true);
    // Every route that does not need a cookie has an expected anonymous answer, and nothing else is listed.
    const credentialless = stubs.filter((route) => !needsCookie(route)).map((route) => `${route.method} ${route.path}`);
    expect([...ANONYMOUS_ANSWERS.keys()].sort()).toEqual(credentialless.sort());
  });

  it.each(stubs.map((route) => [`${route.method} ${route.path}`, route] as const))("%s has a handler: no 501 stub answer with a valid session", async (_name, route) => {
    const res = await send(route, needsCookie(route) ? (await freshAdmin()).cookie : undefined);
    if (STILL_STUBS.has(`${route.method} ${route.path}`)) {
      expect(res.status).toBe(501);
      expect(ApiErrorSchema.parse(await res.json()).code).toBe("unsupported");
      return;
    }
    expect(res.status).not.toBe(501);
    // A session that was refused would pass the check above vacuously: the gates did let this one through.
    if (needsCookie(route)) expect([401, 403, 415, 429]).not.toContain(res.status);
  });

  it.each(stubs.map((route) => [`${route.method} ${route.path}`, route] as const))("%s without a session: 401 on cookie routes, the handler's own answer on public and token routes", async (_name, route) => {
    const res = await send(route);
    if (needsCookie(route)) {
      expect(res.status).toBe(401);
      expect(ApiErrorSchema.parse(await res.json()).code).toBe("unauthorized");
    } else {
      const expected = ANONYMOUS_ANSWERS.get(`${route.method} ${route.path}`);
      expect(expected, "an expected answer for this route").toBeDefined();
      expect(res.status).toBe(expected?.status);
      if (expected?.code) expect(ApiErrorSchema.parse(await res.json()).code).toBe(expected.code);
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
