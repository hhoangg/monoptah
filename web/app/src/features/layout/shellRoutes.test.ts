import { describe, expect, it } from "vitest";
import { routeHasOwnSidebar } from "./shellRoutes";

describe("routeHasOwnSidebar", () => {
  it.each(["/d/dk1", "/d/dk1/", "/d/dk1/p/p1", "/d/dk1/p/p1/", "/d/dk1/cloud"])("is true for %s", (path) => {
    expect(routeHasOwnSidebar(path)).toBe(true);
  });
  it.each(["/d/dk1/p/p1/new", "/d/dk1/p/p1/files/a.ts", "/d/dk1/s/s1", "/d/dk1/monos/m1", "/d/dk1/settings", "/desktops"])("is false for %s", (path) => {
    expect(routeHasOwnSidebar(path)).toBe(false);
  });
});
