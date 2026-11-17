import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { NAVIGATION_DENYLIST, NAVIGATION_FALLBACK, RUNTIME_CACHE_ROUTES, isNeverCached, isShellNavigation } from "./routing";

const swSource = readFileSync(path.resolve(import.meta.dirname, "../sw.ts"), "utf8");

describe("service worker routing config", () => {
  it("falls back to the cached shell", () => {
    expect(NAVIGATION_FALLBACK).toBe("/index.html");
  });

  it.each(["/api/me", "/api/desktops/abc", "/relay/room", "/relay/"])("denies the shell for %s", (pathname) => {
    expect(isShellNavigation(pathname)).toBe(false);
    expect(NAVIGATION_DENYLIST.some((pattern) => pattern.test(pathname))).toBe(true);
  });

  it.each(["/", "/desktops", "/d/dk1/s/s9", "/settings", "/apiary", "/relayed"])("serves the shell for %s", (pathname) => {
    expect(isShellNavigation(pathname)).toBe(true);
  });

  it.each(["/api", "/api/me", "/relay", "/relay/x"])("never caches %s", (pathname) => {
    expect(isNeverCached(pathname)).toBe(true);
  });

  it("has no runtime caching route; if one is ever added, none may match /api or /relay", () => {
    expect(RUNTIME_CACHE_ROUTES).toEqual([]);
    // Guard for a future route: today the list is empty and this loop checks nothing.
    for (const route of RUNTIME_CACHE_ROUTES) {
      expect(route.pattern.test("/api/me")).toBe(false);
      expect(route.pattern.test("/relay/room")).toBe(false);
    }
  });
});

describe("sw.ts wiring", () => {
  it("registers exactly one route, the navigation route with the denylist", () => {
    expect(swSource.match(/registerRoute\(/g)).toHaveLength(1);
    expect(swSource).toContain("new NavigationRoute(");
    expect(swSource).toContain("denylist: [...NAVIGATION_DENYLIST]");
  });

  it("uses no runtime strategy and no fetch handler of its own", () => {
    expect(swSource).not.toMatch(/workbox-strategies|NetworkFirst|CacheFirst|StaleWhileRevalidate|addEventListener\(\s*["']fetch["']/);
  });

  it("does not skip waiting by itself, only on a SKIP_WAITING message", () => {
    const calls = swSource.match(/skipWaiting\(\)/g) ?? [];
    expect(calls).toHaveLength(1);
    expect(swSource).toMatch(/type\s*===\s*"SKIP_WAITING"/);
    expect(swSource).not.toMatch(/addEventListener\(\s*["']install["']/);
  });
});
