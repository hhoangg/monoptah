import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { getLinkConnection, resetLinkConnections } from "./connection.ts";
import { LinkProvider, useLinkConnection, useLinkDesktopId, useLinkState } from "./LinkProvider.tsx";
import { clearLinkStores, getLinkStore } from "./store.ts";

// A fetch that never answers keeps the connection in `connecting` without touching a network.
const pending = (() => new Promise<Response>(() => undefined)) as typeof fetch;

afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
});

function Probe() {
  const id = useLinkDesktopId();
  const status = useLinkState((state) => state.status);
  const connection = useLinkConnection();
  return (
    <p data-testid="probe">
      {id}:{status}:{connection.desktopId}
    </p>
  );
}

describe("LinkProvider", () => {
  it("exposes the desktop id, the state and the connection", () => {
    getLinkConnection("dk1", { fetch: pending, ensureDevice: async () => ({ deviceId: "d", publicKey: new Uint8Array(32), fingerprint: "f" }) });
    render(
      <LinkProvider desktopId="dk1">
        <Probe />
      </LinkProvider>,
    );
    expect(screen.getByTestId("probe")).toHaveTextContent("dk1:connecting:dk1");
  });

  it("throws outside a provider", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    expect(() => render(<Probe />)).toThrow("useLinkDesktopId must be used inside a LinkProvider");
    spy.mockRestore();
  });

  it("two providers for one desktop share the connection and the last unmount stops it", () => {
    const connection = getLinkConnection("dk2", { fetch: pending, ensureDevice: async () => ({ deviceId: "d", publicKey: new Uint8Array(32), fingerprint: "f" }) });
    const a = render(
      <LinkProvider desktopId="dk2">
        <Probe />
      </LinkProvider>,
    );
    const b = render(
      <LinkProvider desktopId="dk2">
        <Probe />
      </LinkProvider>,
    );
    expect(getLinkConnection("dk2")).toBe(connection);
    a.unmount();
    expect(getLinkStore("dk2").getState().status).toBe("connecting");
    b.unmount();
    expect(getLinkStore("dk2").getState().status).toBe("idle");
  });
});

describe("/d/$desktopId route guards", () => {
  function renderRoute(url: string) {
    const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }) });
    render(
      <QueryClientProvider client={createQueryClient()}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    );
    return router;
  }

  it("shows the version screen instead of the outlet for a protocol mismatch", async () => {
    getLinkConnection("dk3", { fetch: pending });
    renderRoute("/d/dk3");
    await screen.findByTestId("route-stub");
    act(() => getLinkStore("dk3").setState({ blocked: "proto_desktop_newer", status: "closed" }));
    expect(await screen.findByText("Reload to update the web app")).toBeInTheDocument();
    act(() => getLinkStore("dk3").setState({ blocked: "proto_desktop_older" }));
    expect(await screen.findByText("Update Monoptah on your desktop")).toBeInTheDocument();
    expect(screen.getByTestId("link-blocked-screen")).toHaveAttribute("data-reason", "proto_desktop_older");
  });

  it("navigates to /login with the current path when login is required", async () => {
    getLinkConnection("dk4", { fetch: pending });
    const router = renderRoute("/d/dk4");
    await screen.findByTestId("route-stub");
    act(() => getLinkStore("dk4").setState({ blocked: "login_required", status: "closed" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/login"));
    expect(router.state.location.search).toMatchObject({ redirect: "/d/dk4" });
  });
});
