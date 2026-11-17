import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { registerLastPlaceResolver, resolveLastPlace } from "./lastPlace";

afterEach(() => registerLastPlaceResolver());

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
  it("goes to /desktops when there is no last place", async () => {
    const router = renderAt("/");
    await screen.findByTestId("route-stub");
    expect(router.state.location.pathname).toBe("/desktops");
  });

  it("goes to the last place of the last used desktop when a resolver is registered", async () => {
    registerLastPlaceResolver(() => "/d/dk1/s/s9");
    const router = renderAt("/");
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/dk1/s/s9"));
    expect((await screen.findByTestId("route-stub")).getAttribute("data-route")).toBe("/d/:desktopId/s/:sessionId");
  });
});
