import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createFakeServer, desktop } from "@/features/desktops/testServer";
import { LAST_DESKTOP_KEY, lastPlaceKey } from "@/features/sessions/lastPlace";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { registerLastPlaceResolver, resolveLastPlace } from "./lastPlace";

let restoreApi: (() => void) | undefined;
beforeEach(() => {
  localStorage.clear();
  // The account holds one desktop, dk1 (`/` checks the last used desktop against the account's list).
  restoreApi = setApiTransport(createFakeServer({ desktops: [desktop({ id: "dk1" })] }).transport);
});
afterEach(() => {
  restoreApi?.();
  localStorage.clear();
  registerLastPlaceResolver();
});

function renderAt(url: string) {
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }) });
  render(
    <QueryClientProvider client={createQueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

describe("last place registry", () => {
  it("defaults to null and ignores resolvers that throw or return a non-path", () => {
    expect(resolveLastPlace()).toBeNull();
    registerLastPlaceResolver(() => {
      throw new Error("boom");
    });
    expect(resolveLastPlace()).toBeNull();
    registerLastPlaceResolver(() => "https://evil.example/x");
    expect(resolveLastPlace()).toBeNull();
    registerLastPlaceResolver(() => "//evil.example/x");
    expect(resolveLastPlace()).toBeNull();
  });

  it("register returns the previous resolver", () => {
    const first = () => "/a";
    registerLastPlaceResolver(first);
    expect(registerLastPlaceResolver(() => "/b")).toBe(first);
  });
});

describe("/ redirect", () => {
  it("drops a last used desktop that is no longer in the account and goes to /settings/desktops (N-12)", async () => {
    localStorage.setItem(LAST_DESKTOP_KEY, "gone");
    localStorage.setItem(lastPlaceKey("gone"), "/d/gone/s/s9");
    registerLastPlaceResolver(() => "/d/gone/s/s9");
    const router = renderAt("/");
    await screen.findByRole("heading", { level: 1, name: "All desktops" });
    expect(router.state.location.pathname).toBe("/settings/desktops");
    expect(localStorage.getItem(LAST_DESKTOP_KEY)).toBeNull();
    expect(localStorage.getItem(lastPlaceKey("gone"))).toBeNull();
  });

  it("still goes to the last place when the desktop list cannot be loaded", async () => {
    restoreApi?.();
    restoreApi = setApiTransport(async () => {
      throw new Error("offline");
    });
    registerLastPlaceResolver(() => "/d/dk1/s/s9");
    const router = renderAt("/");
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/dk1/s/s9"));
  });

  it("goes to /settings/desktops when there is no last place", async () => {
    const router = renderAt("/");
    await screen.findByRole("heading", { level: 1, name: "All desktops" });
    expect(router.state.location.pathname).toBe("/settings/desktops");
  });

  it("goes to the last place of the last used desktop when a resolver is registered", async () => {
    registerLastPlaceResolver(() => "/d/dk1/s/s9");
    const router = renderAt("/");
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/dk1/s/s9"));
    expect((await screen.findByTestId("route-stub")).getAttribute("data-route")).toBe("/d/:desktopId/s/:sessionId");
  });

  it("keeps the search and the hash of the last place (M9)", async () => {
    registerLastPlaceResolver(() => "/d/dk1/s/s9?tab=files#top");
    const router = renderAt("/");
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/dk1/s/s9"));
    expect(router.state.location.searchStr).toBe("?tab=files");
    expect(router.state.location.hash).toBe("top");
  });
});
