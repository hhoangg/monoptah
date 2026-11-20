import { act, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, describe, expect, it } from "vitest";
import { getLinkConnection, resetLinkConnections } from "@/link/connection";
import { LinkProvider } from "@/link/LinkProvider";
import { clearLinkStores, getLinkStore } from "@/link/store";
import { TerminalNavItem } from "./TerminalNavItem";

const pending = (() => new Promise<Response>(() => undefined)) as typeof fetch;

afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
});

async function mount(desktopId: string, children: ReactNode) {
  getLinkConnection(desktopId, { fetch: pending });
  const root = createRootRoute({
    component: () => (
      <LinkProvider desktopId={desktopId}>
        <p data-testid="anchor">mounted</p>
        {children}
      </LinkProvider>
    ),
  });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ["/"] }) });
  render(<RouterProvider router={router} />);
  await screen.findByTestId("anchor");
  return router;
}

const setCapabilities = (desktopId: string, capabilities: string[]) => act(() => getLinkStore(desktopId).setState({ capabilities }));

describe("the Terminal entry of the workspace list", () => {
  it("is hidden until the desktop advertises terminal for this device, and appears and disappears live", async () => {
    await mount("nav-1", <TerminalNavItem desktopId="nav-1" />);
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
    setCapabilities("nav-1", ["cloud", "terminal"]);
    expect(await screen.findByRole("link", { name: "Terminal" })).toHaveAttribute("href", "/d/nav-1/terminal");
    setCapabilities("nav-1", ["cloud"]);
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
  });
  it("stays hidden for a desktop that advertises other capabilities only", async () => {
    await mount("nav-2", <TerminalNavItem desktopId="nav-2" />);
    setCapabilities("nav-2", ["approvals", "cloud", "fileTransfer"]);
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
  });
});
