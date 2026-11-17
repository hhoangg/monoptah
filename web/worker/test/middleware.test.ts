import { Hono } from "hono";
import { describe, expect, it } from "vitest";
import { API_ROUTES, ApiErrorSchema, type ApiRoute } from "@monoptah/protocol";
import { buildCsp, COMPANION_HEADERS } from "../src/middleware/security.ts";
import { ignoreCookies, isCookieAuthenticated } from "../src/middleware/browser.ts";
import type { AppEnv } from "../src/http.ts";
import { BROWSER_HEADERS, call, ORIGIN } from "./helpers.ts";

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

    const ok = await send({ ...BROWSER_HEADERS });
    expect(ok.status).toBe(501);
  });

  it("does not check Origin or CSRF on GET", async () => {
    const res = await call("/api/me");
    expect(res.status).toBe(501);
  });
});

describe("token and pairing routes", () => {
  it.each(tokenOrPairingRoutes.map((r) => [`${r.method} ${r.path}`, r] as const))("%s needs no Origin and no CSRF header, and ignores a cookie", async (_name, route) => {
    const plain = await call(concrete(route), { method: route.method });
    expect(plain.status).toBe(501);
    const withCookie = await call(concrete(route), { method: route.method, headers: { Cookie: "__Host-mc_session=fake-session-token" } });
    expect(withCookie.status).toBe(501);
  });

  it("ignoreCookies removes the Cookie header before a handler can read it", async () => {
    const app = new Hono<AppEnv>();
    app.post("/probe", ignoreCookies, (c) => c.json({ cookie: c.req.header("cookie") ?? null, authorization: c.req.header("authorization") ?? null }));
    const res = await app.request("/probe", { method: "POST", headers: { Cookie: "__Host-mc_session=fake", Authorization: "Bearer fake-token" } });
    expect(await res.json()).toEqual({ cookie: null, authorization: "Bearer fake-token" });
  });
});

describe("response headers", () => {
  it("sets Cache-Control: no-store on /api/* and /relay/* (success, 404 and 501)", async () => {
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
      "Permissions-Policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
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
      "default-src 'none'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self' wss://zptah.app; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; require-trusted-types-for 'script'; trusted-types default; upgrade-insecure-requests",
    );
  });

  it("keeps a port in the host", () => {
    expect(buildCsp("localhost:8787")).toContain("connect-src 'self' wss://localhost:8787;");
  });
});
