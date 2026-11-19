// Headers on what the Worker actually answers for static paths (W13-1, W13-2). The Workers test pool serves
// `app/dist/client` through the real ASSETS binding (a placeholder `index.html` when the app is not built, the
// real build with its `_headers` otherwise), so every assertion here must hold in both cases.
//
// Only a deployed check can show: the edge cache and compression headers added after the Worker, `_headers` rules
// of the Cloudflare assets layer for non-HTML files in production, and the service worker's real `Cache-Control`
// (see docs/security.md 8.1 and the deploy checklist).
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { buildCsp, COMPANION_HEADERS } from "../src/middleware/security.ts";
import { call, callWithEnv, ORIGIN } from "./helpers.ts";

const HOST = new URL(ORIGIN).host;

function expectCompanion(res: Response): void {
  for (const [name, value] of Object.entries(COMPANION_HEADERS)) expect(res.headers.get(name), name).toBe(value);
}

function isHtml(res: Response): boolean {
  return (res.headers.get("Content-Type") ?? "").toLowerCase().startsWith("text/html");
}

describe("a response from ASSETS that already carries a CSP", () => {
  it("gets the Worker's CSP for the request host, not the one of the assets layer (Hono c.res merge)", async () => {
    const conflicting = "default-src 'none'; connect-src 'self' wss://zptah.app";
    const assets = {
      fetch: () =>
        Promise.resolve(
          new Response("<!doctype html><title>x</title>", {
            headers: { "Content-Type": "text/html; charset=utf-8", "Content-Security-Policy": conflicting, "X-Content-Type-Options": "sniff" },
          }),
        ),
    } as unknown as Fetcher;
    const res = await callWithEnv({ ...env, ASSETS: assets } as typeof env, "/some/page");
    expect(res.headers.get("Content-Security-Policy")).toBe(buildCsp(HOST));
    expect(res.headers.get("Content-Security-Policy")).not.toContain("zptah.app");
    expectCompanion(res);
  });
});

describe("static paths through the real assets binding", () => {
  it.each(["/", "/d/some/deep/link", "/login"])("%s: HTML with the Worker CSP and every companion header", async (path) => {
    const res = await call(path);
    expect(res.status).toBe(200);
    expect(isHtml(res)).toBe(true);
    expect(res.headers.get("Content-Security-Policy")).toBe(buildCsp(HOST));
    expectCompanion(res);
  });

  it.each(["/sw.js", "/assets/missing-0123abcd.js"])("%s: companion headers, and the Worker CSP when the body is HTML", async (path) => {
    const res = await call(path);
    expectCompanion(res);
    if (isHtml(res)) expect(res.headers.get("Content-Security-Policy")).toBe(buildCsp(HOST));
  });

  it("answers /api/* without a CSP and with no-store", async () => {
    const res = await call("/api/health");
    expect(res.headers.get("Content-Security-Policy")).toBeNull();
    expect(res.headers.get("Cache-Control")).toBe("no-store");
    expectCompanion(res);
  });
});
