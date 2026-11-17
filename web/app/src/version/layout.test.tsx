import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, loadPersistedCatalog, persistCatalog, registerPersistAdapter, type PersistAdapter } from "@/data/persist";
import { createQueryClient } from "@/lib/queryClient";
import { getLinkConnection, resetLinkConnections } from "@/link/connection";
import { clearLinkStores, getLinkStore } from "@/link/store";
import { createAppRouter } from "@/router";

const pending = (() => new Promise<Response>(() => undefined)) as typeof fetch;
let previous: PersistAdapter;

beforeEach(() => {
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  registerPersistAdapter(previous);
  resetLinkConnections();
  clearLinkStores();
  vi.restoreAllMocks();
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

describe("LinkBlockedScreen in the desktop layout", () => {
  it("a higher proto blocks with the reload message and a Reload button", async () => {
    getLinkConnection("b1", { fetch: pending });
    renderAt("/d/b1");
    await screen.findByTestId("route-stub");
    act(() => getLinkStore("b1").setState({ blocked: "web_outdated", status: "closed" }));
    const screen_ = await screen.findByTestId("link-blocked-screen");
    expect(screen_).toHaveTextContent("Reload to update the web app");
    expect(screen_.textContent).toMatch(/Monoptah/);
    expect(screen_.textContent).not.toMatch(/monocode/i);
    expect(screen.queryByTestId("route-stub")).toBeNull();

    const reload = vi.fn();
    vi.stubGlobal("location", { ...window.location, reload });
    screen.getByRole("button", { name: "Reload" }).click();
    expect(reload).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("unsupported_version (desktop older) blocks with the update message and no Reload button", async () => {
    getLinkConnection("b2", { fetch: pending });
    renderAt("/d/b2");
    await screen.findByTestId("route-stub");
    act(() => getLinkStore("b2").setState({ blocked: "desktop_outdated", status: "closed" }));
    const blocked = await screen.findByTestId("link-blocked-screen");
    expect(blocked).toHaveTextContent("Update Monoptah on your desktop");
    expect(blocked.textContent).not.toMatch(/monocode/i);
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
  });
});

describe("data sync mounted by the layout", () => {
  it("clears the persisted cache on revoked with only the layout mounted", async () => {
    getLinkConnection("r1", { fetch: pending });
    await persistCatalog("r1", { projects: [], sessions: [] });
    renderAt("/d/r1");
    await screen.findByTestId("route-stub");
    expect(await loadPersistedCatalog("r1")).toBeDefined();
    act(() => getLinkStore("r1").setState({ revoked: true, blocked: "revoked", status: "closed" }));
    await waitFor(async () => expect(await loadPersistedCatalog("r1")).toBeUndefined());
  });
});
