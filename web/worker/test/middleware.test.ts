import { env } from "cloudflare:workers";
import { Hono } from "hono";
import { describe, expect, it, vi } from "vitest";
import { API_ROUTES, ApiErrorSchema, type ApiRoute } from "@monoptah/protocol";
import { buildCsp, COMPANION_HEADERS } from "../src/middleware/security.ts";
import { ignoreCookies, isCookieAuthenticated } from "../src/middleware/browser.ts";
import type { AppEnv } from "../src/http.ts";
import { BROWSER_HEADERS, call, callWithEnv, ORIGIN } from "./helpers.ts";
import { createTestUser, loginAs } from "./support/accounts.ts";

/**
 * What a public cookie-less route answers to a request that passes the browser checks but has an empty body: the
 * route's own answer (400 for a bad body, 200 for the body-less login options), never a 401, 403, 415 or 429.
 */
const passedStatus = (route: ApiRoute): number => (route.path === "/api/auth/login/options" ? 200 : 400);

const concrete = (route: ApiRoute) => route.path.replace(/:[A-Za-z]+/g, "x");
const nonGet = (route: ApiRoute) => route.method !== "GET";

// Spec side (architecture.md 4.1, security.md 3.4), written without the implementation's helper.
const cookieRoutes = API_ROUTES.filter(
  (r) => nonGet(r) && (r.auth === "session" || r.auth === "fresh" || r.auth === "admin" || (r.auth === "public" && r.path.startsWith("/api/auth/"))),
);
const tokenOrPairingRoutes = API_ROUTES.filter(
  (r) => nonGet(r) && (r.auth.startsWith("token:") || r.path === "/api/pair/start" || r.path === "/api/pair/poll"),
);

describe("route classification", () => {
  it("is exhaustive: every non-GET route is cookie-authenticated or a token/pairing route", () => {
    const others = API_ROUTES.filter((r) => nonGet(r) && !cookieRoutes.includes(r) && !tokenOrPairingRoutes.includes(r));
    expect(others.map((r) => `${r.method} ${r.path}`)).toEqual([]);
    expect(cookieRoutes.length).toBeGreaterThan(20);
    expect(tokenOrPairingRoutes.length).toBe(5);
  });

  it("matches isCookieAuthenticated on every non-GET route", () => {
    for (const route of API_ROUTES.filter(nonGet)) {
      expect(isCookieAuthenticated(route), `${route.method} ${route.path}`).toBe(cookieRoutes.includes(route));
    }
  });
});

describe("Origin and X-Monoptah-CSRF on cookie routes", () => {
  it.each(cookieRoutes.map((r) => [`${r.method} ${r.path}`, r] as const))("%s", async (_name, route) => {
    const path = concrete(route);
    const send = (headers: Record<string, string>) => call(path, { method: route.method, headers });

    const noOrigin = await send({ "X-Monoptah-CSRF": "1" });
    expect(noOrigin.status).toBe(403);
    expect(ApiErrorSchema.parse(await noOrigin.json()).code).toBe("bad_origin");

    const wrongOrigin = await send({ Origin: "https://evil.example", "X-Monoptah-CSRF": "1" });
    expect(wrongOrigin.status).toBe(403);
    expect(ApiErrorSchema.parse(await wrongOrigin.json()).code).toBe("bad_origin");

    const noCsrf = await send({ Origin: ORIGIN });
    expect(noCsrf.status).toBe(403);
    expect(ApiErrorSchema.parse(await noCsrf.json()).code).toBe("csrf");

    const badCsrf = await send({ Origin: ORIGIN, "X-Monoptah-CSRF": "0" });
    expect(badCsrf.status).toBe(403);
    expect(ApiErrorSchema.parse(await badCsrf.json()).code).toBe("csrf");

    // Passing the browser checks: the request reaches the auth resolver (401 without a session)...
    const ok = await send({ ...BROWSER_HEADERS });
    expect(ok.status).toBe(route.auth === "public" ? passedStatus(route) : 401);
    // ...and with a valid session the handler: whatever it answers to an empty body, it is not a refusal by the
    // middleware (401, 403, 415, 429) and not the old 501 stub.
    if (route.auth !== "public") {
      // A session of its own per route: an earlier route (logout, delete account) may end a shared one.
      const own = await loginAs((await createTestUser(env, { role: "admin" })).id, { fresh: true });
      const authed = await send({ ...BROWSER_HEADERS, Cookie: own.cookie });
      const key = `${route.method} ${route.path}`;
      // Account deletion (ticket U-11) is the one route that is still the 501 stub.
      if (key === "DELETE /api/me") expect(authed.status, key).toBe(501);
      else expect([401, 403, 415, 429, 501], key).not.toContain(authed.status);
    }
  });

  it("does not check Origin or CSRF on GET", async () => {
    // No Origin and no CSRF header: the answer is the resolver's 401, not a 403 bad_origin or csrf.
    const res = await call("/api/me");
    expect(res.status).toBe(401);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("unauthorized");
  });

  it("rejects a non-GET cookie route whose Content-Type is not application/json (415)", async () => {
    const route = cookieRoutes[0] as ApiRoute;
    for (const contentType of [undefined, "text/plain", "application/x-www-form-urlencoded", "multipart/form-data; boundary=x", "application/jsonx"]) {
      const headers: Record<string, string> = { Origin: ORIGIN, "X-Monoptah-CSRF": "1" };
      if (contentType) headers["Content-Type"] = contentType;
      const res = await call(concrete(route), { method: route.method, headers });
      expect(res.status, String(contentType)).toBe(415);
      expect(ApiErrorSchema.parse(await res.json()).code).toBe("unsupported_media_type");
    }
    const withCharset = await call(concrete(route), { method: route.method, headers: { ...BROWSER_HEADERS, "Content-Type": "application/json; charset=utf-8" } });
    expect(withCharset.status).toBe(route.auth === "public" ? passedStatus(route) : 401);
  });

  it("checks Origin on a WebSocket upgrade GET (no CSRF header there)", async () => {
    const path = "/relay/device/x?device=d";
    const upgrade = { Upgrade: "websocket" };
    const none = await call(path, { headers: upgrade });
    expect(none.status).toBe(403);
    expect(ApiErrorSchema.parse(await none.json()).code).toBe("bad_origin");
    const wrong = await call(path, { headers: { ...upgrade, Origin: "https://evil.example" } });
    expect(wrong.status).toBe(403);
    // Right Origin: past the Origin check, the session resolver answers (no cookie: 401).
    const ok = await call(path, { headers: { ...upgrade, Origin: ORIGIN } });
    expect(ok.status).toBe(401);
  });
});

describe("token and pairing routes", () => {
  it.each(tokenOrPairingRoutes.map((r) => [`${r.method} ${r.path}`, r] as const))("%s needs no Origin and no CSRF header, and ignores a cookie", async (_name, route) => {
    // Past the browser checks (none apply) the handler answers: pairing validates its body (400 invalid), token routes
    // want a desktop token (401 unauthorized). A cookie changes neither answer.
    const expected = route.path.startsWith("/api/pair/") ? { status: 400, code: "invalid" } : { status: 401, code: "unauthorized" };
    const plain = await call(concrete(route), { method: route.method });
    expect(plain.status).toBe(expected.status);
    expect(ApiErrorSchema.parse(await plain.json()).code).toBe(expected.code);
    const withCookie = await call(concrete(route), { method: route.method, headers: { Cookie: "__Host-mc_session=fake-session-token" } });
    expect(withCookie.status).toBe(expected.status);
    expect(ApiErrorSchema.parse(await withCookie.json()).code).toBe(expected.code);
  });

  it("ignoreCookies removes the Cookie header before a handler can read it", async () => {
    const app = new Hono<AppEnv>();
    app.post("/probe", ignoreCookies, (c) => c.json({ cookie: c.req.header("cookie") ?? null, authorization: c.req.header("authorization") ?? null }));
    const res = await app.request("/probe", { method: "POST", headers: { Cookie: "__Host-mc_session=fake", Authorization: "Bearer fake-token" } });
    expect(await res.json()).toEqual({ cookie: null, authorization: "Bearer fake-token" });
  });
});

describe("response headers", () => {
  it("sets Cache-Control: no-store on /api/* and /relay/* (success, 404 and handler errors)", async () => {
    for (const path of ["/api/health", "/api/nope", "/api/me", "/relay/desktop/x", "/relay/nope"]) {
      const res = await call(path);
      expect(res.headers.get("Cache-Control"), path).toBe("no-store");
    }
  });

  it("sets the companion headers of security.md 8.1", async () => {
    for (const path of ["/api/health", "/api/me", "/"]) {
      const res = await call(path);
      for (const [name, value] of Object.entries(COMPANION_HEADERS)) expect(res.headers.get(name), `${path} ${name}`).toBe(value);
    }
    expect(COMPANION_HEADERS).toEqual({
      "Strict-Transport-Security": "max-age=63072000; includeSubDomains; preload",
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "no-referrer",
      "Permissions-Policy": "camera=(), microphone=(self), geolocation=(), payment=(), usb=()",
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Resource-Policy": "same-origin",
    });
  });

  it("does not set no-store on a non-API path", async () => {
    const res = await call("/");
    expect(res.headers.get("Cache-Control")).not.toBe("no-store");
  });

  it("puts the CSP on HTML the Worker returns, with the request host, and not on JSON", async () => {
    const html = await call("/");
    expect(html.headers.get("Content-Type")).toContain("text/html");
    expect(html.headers.get("Content-Security-Policy")).toBe(buildCsp(new URL(ORIGIN).host));
    const json = await call("/api/health");
    expect(json.headers.get("Content-Security-Policy")).toBeNull();
  });

  it("serves the SPA for an unknown non-API path (single-page-application fallback)", async () => {
    const res = await call("/d/some/deep/link");
    expect(res.status).toBe(200);
    expect(res.headers.get("Content-Type")).toContain("text/html");
  });
});

describe("buildCsp", () => {
  it("is exactly the policy of security.md 8.1 with HOST substituted", () => {
    expect(buildCsp("zptah.app")).toBe(
      "default-src 'none'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; frame-src blob:; font-src 'self'; connect-src 'self' wss://zptah.app; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; require-trusted-types-for 'script'; trusted-types default; upgrade-insecure-requests",
    );
  });

  it("keeps a port in the host", () => {
    expect(buildCsp("localhost:8787")).toContain("connect-src 'self' wss://localhost:8787;");
  });
});

describe("the 500 log line", () => {
  it("holds the route pattern, never a path parameter such as a pairing code", async () => {
    const broken = Object.create(env) as typeof env;
    Object.defineProperty(broken, "DB", {
      value: {
        prepare: () => {
          throw new Error("boom with PAIRSECRET1 inside");
        },
      } as unknown as D1Database,
    });
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const res = await callWithEnv(broken, "/api/pair/PAIRSECRET1", { headers: { Cookie: "__Host-mc_session=whatever" } });
      expect(res.status).toBe(500);
      const logged = errors.mock.calls.map((args) => String(args[0]));
      expect(logged).toEqual(["unhandled error: route=GET /api/pair/:userCode error=Error"]);
      expect(logged.join(" ")).not.toContain("PAIRSECRET1");
    } finally {
      errors.mockRestore();
    }
  });
});
