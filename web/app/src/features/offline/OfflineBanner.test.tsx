import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { clearLinkStores } from "@/link/store";
import { getLinkStore } from "@/link";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { desktopIdFromPath } from "./OfflineBanner";

function setOnline(value: boolean) {
  vi.spyOn(navigator, "onLine", "get").mockReturnValue(value);
  window.dispatchEvent(new Event(value ? "online" : "offline"));
}

async function renderAt(url: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return screen.findByTestId("connection-banner");
}

afterEach(() => {
  vi.restoreAllMocks();
  clearLinkStores();
});

describe("desktopIdFromPath", () => {
  it("reads the id of a desktop route only", () => {
    expect(desktopIdFromPath("/d/dk1")).toBe("dk1");
    expect(desktopIdFromPath("/d/dk1/s/s9")).toBe("dk1");
    expect(desktopIdFromPath("/desktops/dk1")).toBeNull();
    expect(desktopIdFromPath("/settings")).toBeNull();
  });
});

describe("OfflineBanner", () => {
  it("is an empty polite live region while online", async () => {
    const region = await renderAt("/settings");
    expect(region).toHaveAttribute("aria-live", "polite");
    expect(region).toBeEmptyDOMElement();
  });

  it("announces You are offline and goes away when the connection returns", async () => {
    setOnline(false);
    const region = await renderAt("/settings");
    expect(region).toHaveTextContent("You are offline");
    act(() => setOnline(true));
    expect(region).toBeEmptyDOMElement();
  });

  it("shows Reconnecting... with Retry on a desktop route", async () => {
    getLinkStore("dk1").setState({ status: "reconnecting" });
    const region = await renderAt("/d/dk1");
    expect(region).toHaveTextContent("Reconnecting...");
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("shows Desktop offline when the relay says the desktop is gone", async () => {
    getLinkStore("dk1").setState({ status: "ready", presence: "offline", lastFrameAt: Date.now() - 5 * 60_000 });
    const region = await renderAt("/d/dk1");
    expect(region).toHaveTextContent("Desktop offline");
    expect(region).toHaveTextContent("last seen 5 min ago");
  });

  it("shows Desktop offline with the directory last seen when the desktop was offline at page open", async () => {
    const desktop = { id: "dk1", name: "Mac", color: null, online: false, pendingCount: 0, os: null, appVersion: null, lastSeenAt: Date.now() - 2 * 3_600_000 };
    const mock = createMockServer({ signedIn: true });
    const restore = setApiTransport(async (url, init) =>
      url === "/api/desktops" ? new Response(JSON.stringify({ desktops: [desktop] }), { status: 200, headers: { "Content-Type": "application/json" } }) : mock.transport(url, init),
    );
    getLinkStore("dk1").setState({ status: "reconnecting", presence: "offline", lastFrameAt: null });
    const region = await renderAt("/d/dk1");
    expect(region).toHaveTextContent("Desktop offline");
    expect(screen.queryByText("Reconnecting...")).toBeNull();
    await vi.waitFor(() => expect(region).toHaveTextContent("last seen 2 h ago"));
    restore();
  });

  it("ignores the link state of a desktop that is not on screen", async () => {
    getLinkStore("dk2").setState({ status: "reconnecting" });
    const region = await renderAt("/d/dk1");
    expect(region).toBeEmptyDOMElement();
  });
});
