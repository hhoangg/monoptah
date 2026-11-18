import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { registerLastPlaceResolver, resolveLastPlace } from "@/data";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { LAST_DESKTOP_KEY, forgetDesktop, lastPlaceKey, placeDesktop, readLastPlace, rememberPlace } from "./lastPlace";
import "./register";

beforeEach(() => localStorage.clear());
afterEach(() => localStorage.clear());

describe("last place (F-14)", () => {
  it("remembers workspace places per desktop and the last used desktop", () => {
    rememberPlace("/d/dk1/p/proj-alpha");
    rememberPlace("/d/dk2/s/sess-9?x=1");
    expect(localStorage.getItem(lastPlaceKey("dk1"))).toBe("/d/dk1/p/proj-alpha");
    expect(localStorage.getItem(LAST_DESKTOP_KEY)).toBe("dk2");
    expect(readLastPlace()).toBe("/d/dk2/s/sess-9?x=1");
  });

  it("does not remember settings, cloud or other routes", () => {
    rememberPlace("/d/dk1/settings");
    rememberPlace("/d/dk1/cloud");
    rememberPlace("/desktops");
    expect(localStorage.getItem(LAST_DESKTOP_KEY)).toBeNull();
    expect(readLastPlace()).toBeNull();
  });

  it("remembers a desktop's home and falls back to it when only the desktop is known", () => {
    rememberPlace("/d/dk1");
    expect(readLastPlace()).toBe("/d/dk1");
    localStorage.removeItem(lastPlaceKey("dk1"));
    expect(readLastPlace()).toBe("/d/dk1");
  });

  it("ignores a stored place that belongs to another desktop", () => {
    localStorage.setItem(LAST_DESKTOP_KEY, "dk1");
    localStorage.setItem(lastPlaceKey("dk1"), "/d/other/p/x");
    expect(readLastPlace()).toBe("/d/dk1");
  });

  it("forgets a desktop", () => {
    rememberPlace("/d/dk1/p/a");
    forgetDesktop("dk1");
    expect(readLastPlace()).toBeNull();
  });

  it("parses the desktop id", () => {
    expect(placeDesktop("/d/dk%201/p/a")).toBe("dk 1");
    expect(placeDesktop("/d/")).toBeNull();
  });

  it("is registered as the resolver of `/` by register.ts", () => {
    expect(resolveLastPlace()).toBeNull();
    rememberPlace("/d/dk1/p/proj-alpha");
    expect(resolveLastPlace()).toBe("/d/dk1/p/proj-alpha");
  });

  it("`/` redirects to the last place", async () => {
    rememberPlace("/d/dk1/p/proj-alpha");
    const router = createAppRouter({ history: createMemoryHistory({ initialEntries: ["/"] }) });
    render(
      <QueryClientProvider client={createQueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    await router.load();
    expect(router.state.location.pathname).toBe("/d/dk1/p/proj-alpha");
    registerLastPlaceResolver(readLastPlace);
  });
});
