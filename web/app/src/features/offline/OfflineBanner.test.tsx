import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import type { ComponentProps } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { clearLinkStores } from "@/link/store";
import { getLinkStore } from "@/link";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { DesktopSwitcher } from "@/features/desktops/DesktopSwitcher";
import { clearSlots, registerSlot } from "@/ui/slots";
import { OfflineBanner, SHOW_DELAY_MS, desktopIdFromPath } from "./OfflineBanner";

function setOnline(value: boolean) {
  vi.spyOn(navigator, "onLine", "get").mockReturnValue(value);
  window.dispatchEvent(new Event(value ? "online" : "offline"));
}

const sleep = (ms: number) => act(() => new Promise<void>((resolve) => setTimeout(resolve, ms)));

async function renderAt(url: string) {
  // On a desktop route the banner sits under the switcher row (DesktopTopBar), which the switcher slot renders.
  registerSlot("desktop-switcher", DesktopSwitcher);
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return screen.findByTestId("connection-banner");
}

/** Mounts the banner alone (outside the app shell) in a router, to check the classes of each `pinned` option. */
async function renderBanner(props: ComponentProps<typeof OfflineBanner>) {
  const queryClient = createQueryClient();
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <OfflineBanner {...props} /> }),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return screen.findByTestId("connection-banner");
}

afterEach(() => {
  clearSlots();
  vi.restoreAllMocks();
  // A test that ended offline left TanStack Query's onlineManager offline (it follows the window events), which pauses
  // every query of the next test (the auth guard never answers, so the router renders nothing). Tell it we are back.
  window.dispatchEvent(new Event("online"));
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

  it("is an overlay: zero height in the flow, no pointer events except on the pill", async () => {
    getLinkStore("dk1").setState({ status: "reconnecting" });
    const region = await renderAt("/d/dk1");
    expect(region).toHaveClass("h-0", "pointer-events-none");
    await vi.waitFor(() => expect(region).toHaveTextContent("Reconnecting..."));
    expect(region.querySelector("[data-kind]")).toHaveClass("pointer-events-auto");
  });

  it("announces You are offline after the delay and goes away when the connection returns", async () => {
    setOnline(false);
    const region = await renderAt("/settings");
    expect(region).toBeEmptyDOMElement();
    await vi.waitFor(() => expect(region).toHaveTextContent("You are offline - showing saved data"));
    expect(region).toHaveAttribute("aria-live", "polite");
    act(() => setOnline(true));
    await vi.waitFor(() => expect(region).toBeEmptyDOMElement());
  });

  it("stays pinned under the sticky header on a page that scrolls", async () => {
    const region = await renderAt("/settings");
    expect(region).toHaveClass("sticky");
    expect(region.className).toContain("safe-area-inset-top");
  });

  it("sticks to the top edge (notch inset only) when the page has no app header, as on settings", async () => {
    const region = await renderBanner({ pinned: "top" });
    expect(region).toHaveClass("sticky", "top-[env(safe-area-inset-top)]");
    expect(region.className).not.toContain("2.75rem");
  });

  it("marks the document while a pinned toast shows, and clears it after and on unmount", async () => {
    const root = document.documentElement;
    const region = await renderBanner({ pinned: "top" });
    expect(root).not.toHaveAttribute("data-connection-banner");
    act(() => setOnline(false));
    await vi.waitFor(() => expect(region).toHaveTextContent("You are offline"));
    expect(root).toHaveAttribute("data-connection-banner", "shown");
    act(() => setOnline(true));
    await vi.waitFor(() => expect(root).not.toHaveAttribute("data-connection-banner"));
    act(() => setOnline(false));
    await vi.waitFor(() => expect(root).toHaveAttribute("data-connection-banner", "shown"));
    cleanup();
    expect(root).not.toHaveAttribute("data-connection-banner");
  });

  it("publishes the measured pill height as --connection-banner-height while a pinned toast shows, and removes it after", async () => {
    const root = document.documentElement;
    const callbacks: (() => void)[] = [];
    const observed: Element[] = [];
    let disconnects = 0;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: () => void) {
          callbacks.push(callback);
        }
        observe(target: Element) {
          observed.push(target);
        }
        disconnect() {
          disconnects += 1;
        }
      },
    );
    // Only the pill (the element with data-kind) has a height; every other element measures 0.
    let pillHeight = 58;
    const original = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return this.hasAttribute("data-kind") ? ({ height: pillHeight } as DOMRect) : original.call(this);
    });
    try {
      const region = await renderBanner({ pinned: "top" });
      expect(root.style.getPropertyValue("--connection-banner-height")).toBe("");
      act(() => setOnline(false));
      await vi.waitFor(() => expect(region).toHaveTextContent("You are offline"));
      await vi.waitFor(() => expect(root.style.getPropertyValue("--connection-banner-height")).toBe("58px"));
      expect(observed).toHaveLength(1);
      expect(observed[0]).toHaveAttribute("data-kind");
      pillHeight = 80;
      act(() => callbacks.forEach((callback) => callback()));
      expect(root.style.getPropertyValue("--connection-banner-height")).toBe("80px");
      expect(disconnects).toBe(0);
      act(() => setOnline(true));
      await vi.waitFor(() => expect(root.style.getPropertyValue("--connection-banner-height")).toBe(""));
      expect(disconnects).toBe(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("removes --connection-banner-height and disconnects the observer when unmounted while the toast shows", async () => {
    const root = document.documentElement;
    let disconnects = 0;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {
          disconnects += 1;
        }
      },
    );
    const original = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return this.hasAttribute("data-kind") ? ({ height: 58 } as DOMRect) : original.call(this);
    });
    try {
      const region = await renderBanner({ pinned: "top" });
      act(() => setOnline(false));
      await vi.waitFor(() => expect(region).toHaveTextContent("You are offline"));
      await vi.waitFor(() => expect(root.style.getPropertyValue("--connection-banner-height")).toBe("58px"));
      cleanup();
      expect(root.style.getPropertyValue("--connection-banner-height")).toBe("");
      expect(disconnects).toBeGreaterThanOrEqual(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("does not mark the document for a banner that is not pinned to the top", async () => {
    const region = await renderBanner({});
    act(() => setOnline(false));
    await vi.waitFor(() => expect(region).toHaveTextContent("You are offline"));
    expect(document.documentElement).not.toHaveAttribute("data-connection-banner");
  });

  it("keeps pinnedBelowHeader working, and is not pinned by default", async () => {
    expect(await renderBanner({ pinnedBelowHeader: true })).toHaveClass("sticky", "top-[calc(env(safe-area-inset-top)+2.75rem)]");
    cleanup();
    expect(await renderBanner({})).toHaveClass("relative");
  });

  it("shows nothing before the show delay", async () => {
    const region = await renderAt("/settings");
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      act(() => setOnline(false));
      act(() => void vi.advanceTimersByTime(SHOW_DELAY_MS - 50));
      expect(region).toBeEmptyDOMElement();
      act(() => void vi.advanceTimersByTime(100));
      expect(region).toHaveTextContent("You are offline");
      act(() => setOnline(true));
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not flash on a quick reconnect", async () => {
    const region = await renderAt("/settings");
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      act(() => setOnline(false));
      act(() => void vi.advanceTimersByTime(SHOW_DELAY_MS - 100));
      act(() => setOnline(true));
      act(() => void vi.advanceTimersByTime(SHOW_DELAY_MS * 3));
      expect(region).toBeEmptyDOMElement();
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows Reconnecting... with Retry on a desktop route", async () => {
    getLinkStore("dk1").setState({ status: "reconnecting" });
    const region = await renderAt("/d/dk1");
    await vi.waitFor(() => expect(region).toHaveTextContent("Reconnecting... - showing saved data"));
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("says Connecting... on a cold start, without Retry", async () => {
    getLinkStore("dk1").setState({ status: "connecting" });
    const region = await renderAt("/d/dk1");
    await vi.waitFor(() => expect(region).toHaveTextContent("Connecting... - showing saved data"));
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("shows Desktop offline with the Retry action when the relay says the desktop is gone", async () => {
    getLinkStore("dk1").setState({ status: "ready", presence: "offline", lastFrameAt: Date.now() - 5 * 60_000 });
    const region = await renderAt("/d/dk1");
    await vi.waitFor(() => expect(region).toHaveTextContent("Desktop offline"));
    expect(region).toHaveTextContent("last seen 5 min ago");
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("shows Desktop offline with the directory last seen when the desktop was offline at page open", async () => {
    const desktop = { id: "dk1", name: "Mac", color: null, online: false, pendingCount: 0, os: null, appVersion: null, lastSeenAt: Math.floor(Date.now() / 1000) - 2 * 3600 };
    const mock = createMockServer({ signedIn: true });
    const restore = setApiTransport(async (url, init) =>
      url === "/api/desktops" ? new Response(JSON.stringify({ desktops: [desktop] }), { status: 200, headers: { "Content-Type": "application/json" } }) : mock.transport(url, init),
    );
    getLinkStore("dk1").setState({ status: "reconnecting", presence: "offline", lastFrameAt: null });
    const region = await renderAt("/d/dk1");
    await vi.waitFor(() => expect(region).toHaveTextContent("Desktop offline"));
    expect(screen.queryByText("Reconnecting...")).toBeNull();
    await vi.waitFor(() => expect(region).toHaveTextContent("last seen 2 h ago"));
    restore();
  });

  it("ignores the link state of a desktop that is not on screen", async () => {
    getLinkStore("dk2").setState({ status: "reconnecting" });
    const region = await renderAt("/d/dk1");
    await sleep(SHOW_DELAY_MS + 100);
    expect(region).toBeEmptyDOMElement();
  });
});
