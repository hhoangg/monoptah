// The exit button of the settings shell (no app header there): where it goes, and where it shows.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createMockServer } from "@/features/accounts/mock/transport";
import { desktopsListQuery } from "@/features/desktops/api";
import { touchLastUsed } from "@/features/desktops/storage";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { LAST_DESKTOP_KEY } from "@/features/sessions/lastPlace";

const DESKTOP = "desktop-dev-1";

let lastQueryClient = createQueryClient();

function mount(initialEntries: string[], history = createMemoryHistory({ initialEntries })) {
  const queryClient = createQueryClient();
  lastQueryClient = queryClient;
  const router = createAppRouter({ history, queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

/** No width query matches: a narrow window, so /settings stays the list page. */
function stubNarrow() {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  }));
}

afterEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("settings exit button", () => {
  it("has an accessible name, is a plain button and is a 44px circle", async () => {
    mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const button = within(nav).getByRole("button", { name: "Leave settings" });
    expect(button).toHaveAttribute("type", "button");
    expect(button).toHaveClass("size-11", "rounded-pill");
  });

  it("starts the wide nav column: before the quick-find box, with the title text but no extra heading", async () => {
    mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const button = within(nav).getByRole("button", { name: "Leave settings" });
    const row = button.parentElement as HTMLElement;
    expect(row).toHaveClass("hidden", "lg:flex");
    expect(row).toHaveTextContent("Settings");
    expect(nav.firstElementChild).toBe(row);
    expect(row.compareDocumentPosition(within(nav).getByRole("searchbox", { name: "Find a setting" })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(row).queryByRole("heading")).toBeNull();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("is also the first control of the narrow list page header, before its h1", async () => {
    stubNarrow();
    mount(["/settings"]);
    const h1 = await screen.findByRole("heading", { level: 1, name: "Settings" });
    const header = h1.closest("header") as HTMLElement;
    expect(header).toHaveClass("lg:hidden");
    const button = within(header).getByRole("button", { name: "Leave settings" });
    expect(button.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  /** Opens settings from a desktop page by navigating, so the history log sees every entry. */
  async function enterSettings(router: ReturnType<typeof mount>) {
    await router.load();
    await router.navigate({ to: "/settings/profile" });
    return screen.findByRole("navigation", { name: "Settings" });
  }

  it("goes back to the entry before settings, also after visiting two sub pages", async () => {
    const router = mount([`/d/${DESKTOP}`]);
    const nav = await enterSettings(router);
    const user = userEvent.setup();
    await user.click(within(nav).getByRole("link", { name: "Security" }));
    await screen.findByRole("heading", { level: 1, name: "Security" });
    await user.click(within(nav).getByRole("link", { name: "Invites" }));
    await screen.findByRole("heading", { level: 1, name: "Invites" });
    expect(router.history.location.state.__TSR_index).toBe(3);
    const go = vi.spyOn(router.history, "go");
    await user.click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}`));
    expect(router.history.location.state.__TSR_index).toBe(0);
    expect(go).toHaveBeenCalledTimes(1);
    expect(go).toHaveBeenCalledWith(-3);
  });

  it("still goes to the entry before settings after a reload (a fresh shell, the log restored from sessionStorage)", async () => {
    const history = createMemoryHistory({ initialEntries: [`/d/${DESKTOP}`] });
    const first = mount([], history);
    await enterSettings(first);
    await first.navigate({ to: "/settings/security" });
    await screen.findByRole("heading", { level: 1, name: "Security" });
    expect(history.location.state.__TSR_index).toBe(2);
    cleanup();
    // A reload: new router and shell on the same history entry (index 2); only sessionStorage survives.
    const second = mount([], history);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const go = vi.spyOn(second.history, "go");
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(second.state.location.pathname).toBe(`/d/${DESKTOP}`));
    expect(go).toHaveBeenCalledWith(-2);
  });

  it("goes to the entry before settings after a Back and Forward re-entry into settings", async () => {
    const router = mount([`/d/${DESKTOP}`]);
    const nav = await enterSettings(router);
    const user = userEvent.setup();
    await user.click(within(nav).getByRole("link", { name: "Security" }));
    await screen.findByRole("heading", { level: 1, name: "Security" });
    router.history.back();
    await screen.findByRole("heading", { level: 1, name: "Profile" });
    router.history.forward();
    await screen.findByRole("heading", { level: 1, name: "Security" });
    const go = vi.spyOn(router.history, "go");
    await user.click(within(await screen.findByRole("navigation", { name: "Settings" })).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}`));
    expect(go).toHaveBeenCalledWith(-2);
  });

  it("goes to the entry just before settings after a multi-step Back and Forward jump", async () => {
    const router = mount([`/d/${DESKTOP}`]);
    await router.load();
    await router.navigate({ to: "/d/$desktopId/p/$projectId", params: { desktopId: DESKTOP, projectId: "x" } });
    await router.navigate({ to: "/settings/profile" });
    await screen.findByRole("navigation", { name: "Settings" });
    expect(router.history.location.state.__TSR_index).toBe(2);
    router.history.go(-2);
    await waitFor(() => expect(router.history.location.state.__TSR_index).toBe(0));
    router.history.go(2);
    await screen.findByRole("heading", { level: 1, name: "Profile" });
    const go = vi.spyOn(router.history, "go");
    await userEvent.setup().click(within(await screen.findByRole("navigation", { name: "Settings" })).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.history.location.state.__TSR_index).toBe(1));
    expect(go).toHaveBeenCalledWith(-1);
  });

  it("skips a sign-in page between the desktop and settings", async () => {
    const router = mount([`/d/${DESKTOP}`]);
    await router.load();
    await router.navigate({ to: "/login" });
    await router.navigate({ to: "/settings/profile" });
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const go = vi.spyOn(router.history, "go");
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}`));
    expect(go).toHaveBeenCalledWith(-2);
  });

  it("falls back, with a replacing navigation and no history call, when only sign-in pages come before settings", async () => {
    const router = mount(["/login"]);
    await router.load();
    await router.navigate({ to: "/settings/profile" });
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const go = vi.spyOn(router.history, "go");
    const navigate = vi.spyOn(router, "navigate");
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    expect(go).not.toHaveBeenCalled();
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ to: "/d/$desktopId", params: { desktopId: "desktop-dev-1" }, replace: true })));
    expect(navigate).not.toHaveBeenCalledWith(expect.objectContaining({ to: "/" }));
  });

  it("never calls go when settings was opened directly (no entry before it), and replaces instead", async () => {
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const go = vi.spyOn(router.history, "go");
    const navigate = vi.spyOn(router, "navigate");
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}`));
    expect(go).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ replace: true }));
  });

  it("goes to the last place desktop, which is neither the first nor the most recently used one", async () => {
    setApiTransport(createMockServer({ signedIn: true, desktopCount: 3 }).transport);
    localStorage.setItem(LAST_DESKTOP_KEY, "desktop-dev-3");
    touchLastUsed("desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await within(nav).findByRole("link", { name: /Fake Studio Mac/ });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-3"));
  });

  it("goes to the first desktop when settings was opened directly and there is no last place or last used desktop", async () => {
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}`));
  });

  it("goes to the most recently used desktop when there is no last place", async () => {
    touchLastUsed("desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"));
  });

  it("goes to the Pair a desktop page when the account has no desktop, not back into settings", async () => {
    setApiTransport(createMockServer({ signedIn: true, desktopCount: 0 }).transport);
    const router = mount(["/settings/profile"]);
    const navigate = vi.spyOn(router, "navigate");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await screen.findByRole("heading", { level: 1, name: "Pair a desktop" });
    expect(router.state.location.pathname).toBe("/pair");
    expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ to: "/pair", replace: true }));
  });

  it("also goes to the Pair a desktop page when the only entry before settings is a sign-in page", async () => {
    setApiTransport(createMockServer({ signedIn: true, desktopCount: 0 }).transport);
    const router = mount(["/login"]);
    await router.load();
    await router.navigate({ to: "/settings/profile" });
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await screen.findByRole("heading", { level: 1, name: "Pair a desktop" });
  });

  /** The desktops list fails, every other call is answered by the mock. */
  function failDesktopsList() {
    const server = createMockServer({ signedIn: true });
    setApiTransport(async (url, init) => (url === "/api/desktops" ? new Response("{}", { status: 500 }) : server.transport(url, init)));
  }

  /** The desktops list request never answers, every other call is answered by the mock. */
  function hangDesktopsList() {
    const server = createMockServer({ signedIn: true });
    setApiTransport((url, init) => (url === "/api/desktops" ? new Promise<Response>(() => undefined) : server.transport(url, init)));
  }

  /** Waits until the nav's own request for the list has failed once (it is now waiting to retry). */
  const listFailed = () => waitFor(() => expect(lastQueryClient.getQueryState(desktopsListQuery.queryKey)?.fetchFailureCount ?? 0).toBeGreaterThan(0));

  it("goes to the Pair a desktop page, outside settings, when the desktops list cannot be loaded and there is no last place", async () => {
    failDesktopsList();
    const router = mount(["/settings/profile"]);
    const navigate = vi.spyOn(router, "navigate");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await listFailed();
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    // Well under the nav's 1s retry delay: the exit does not wait for the shared request.
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ to: "/pair", replace: true })), { timeout: 800 });
    await waitFor(() => expect(router.state.location.pathname).toBe("/pair"), { timeout: 800 });
    expect(router.state.location.pathname.startsWith("/settings")).toBe(false);
    expect(navigate).not.toHaveBeenCalledWith(expect.objectContaining({ to: "/" }));
  });

  it("uses the cached desktops list when a later load fails, with the same rules", async () => {
    const server = createMockServer({ signedIn: true });
    let failing = false;
    setApiTransport(async (url, init) => (url === "/api/desktops" && failing ? new Response("{}", { status: 500 }) : server.transport(url, init)));
    touchLastUsed("desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await within(nav).findByRole("link", { name: /Fake Studio Mac/ });
    failing = true;
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"));
  });

  it("navigates at once from the cached list while offline", async () => {
    touchLastUsed("desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await within(nav).findByRole("link", { name: /Fake Studio Mac/ });
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    window.dispatchEvent(new Event("offline"));
    try {
      await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
      await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"));
    } finally {
      window.dispatchEvent(new Event("online"));
    }
  });

  it("drops the navigation when the location changes while the desktops list is loading", async () => {
    const server = createMockServer({ signedIn: true });
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    setApiTransport(async (url, init) => {
      if (url === "/api/desktops") await gate;
      return server.transport(url, init);
    });
    const router = mount(["/settings/profile"]);
    const navigate = vi.spyOn(router, "navigate");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await router.navigate({ to: "/settings/security" });
    navigate.mockClear();
    release();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(navigate).not.toHaveBeenCalled();
    expect(router.state.location.pathname).toBe("/settings/security");
  });

  it("ignores a second click before the router resolves (one history jump, no overshoot)", async () => {
    const router = mount([`/d/${DESKTOP}`]);
    await router.load();
    await router.navigate({ to: "/d/$desktopId/p/$projectId", params: { desktopId: DESKTOP, projectId: "x" } });
    await router.navigate({ to: "/settings/profile" });
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const go = vi.spyOn(router.history, "go");
    const navigate = vi.spyOn(router, "navigate");
    const button = within(nav).getByRole("button", { name: "Leave settings" });
    act(() => {
      fireEvent.click(button);
      fireEvent.click(button);
    });
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}/p/x`));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(go).toHaveBeenCalledTimes(1);
    expect(go).toHaveBeenCalledWith(-1);
    expect(router.state.location.pathname).toBe(`/d/${DESKTOP}/p/x`);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("ignores a second click on the fallback path (one navigation)", async () => {
    const router = mount(["/settings/profile"]);
    const navigate = vi.spyOn(router, "navigate");
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const button = within(nav).getByRole("button", { name: "Leave settings" });
    act(() => {
      fireEvent.click(button);
      fireEvent.click(button);
    });
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP}`));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(navigate.mock.calls.filter(([options]) => (options as { to?: string }).to === "/d/$desktopId")).toHaveLength(1);
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it("leaves at once offline with no cached list, to the last place desktop, and a later click still works", async () => {
    // The list never loads: a hanging request, as offline.
    hangDesktopsList();
    localStorage.setItem(LAST_DESKTOP_KEY, "desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    window.dispatchEvent(new Event("offline"));
    try {
      await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
      await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"), { timeout: 800 });
      // Not stuck: back in settings, the button works again.
      await router.navigate({ to: "/settings/profile" });
      const again = within(await screen.findByRole("navigation", { name: "Settings" })).getByRole("button", { name: "Leave settings" });
      await userEvent.setup().click(again);
      await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"), { timeout: 800 });
    } finally {
      window.dispatchEvent(new Event("online"));
    }
  });

  it("leaves offline with no cached list and no last place to the Pair a desktop page", async () => {
    hangDesktopsList();
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    window.dispatchEvent(new Event("offline"));
    try {
      await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
      await waitFor(() => expect(router.state.location.pathname).toBe("/pair"), { timeout: 800 });
    } finally {
      window.dispatchEvent(new Event("online"));
    }
  });

  it("goes to the last place desktop when the desktops list cannot be loaded", async () => {
    failDesktopsList();
    localStorage.setItem(LAST_DESKTOP_KEY, "desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await listFailed();
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"), { timeout: 800 });
  });

  it("skips a last used desktop that left the account's list and opens the most recently used one", async () => {
    localStorage.setItem(LAST_DESKTOP_KEY, "desktop-gone");
    touchLastUsed("desktop-dev-2");
    const router = mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await within(nav).findByRole("link", { name: /Fake Studio Mac/ });
    await userEvent.setup().click(within(nav).getByRole("button", { name: "Leave settings" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/d/desktop-dev-2"));
  });
});

describe("settings nav rows", () => {
  it("use the shared card inset, the divider from the text start and the hide-around-highlight variants below lg", async () => {
    mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    const list = within(nav).getByRole("list", { name: /^Account/ });
    expect(list).toHaveClass("settings-nav-card", "max-lg:rounded-card", "max-lg:bg-surface", "max-lg:p-[var(--settings-card-inset)]");
    const item = within(list).getByRole("link", { name: "Profile" }).closest("li") as HTMLElement;
    expect(item).toHaveClass("max-lg:before:left-[54px]", "max-lg:before:right-1.5", "[@media(hover:hover)]:has-[>[data-row-highlight]:hover]:before:opacity-0", "has-[>[data-row-highlight]:is(:focus-visible,[aria-current=page])]:before:opacity-0");
    // From lg there is no card and no divider: a compact row with the same 12px radius and equal side padding.
    const link = within(item).getByRole("link", { name: "Profile" });
    expect(link).toHaveClass("rounded-row", "lg:px-2.5", "lg:pointer-fine:px-2");
    expect(link).toHaveAttribute("data-row-highlight");
  });

  it("gives a quick-find result a divider at the text inset (no icon)", async () => {
    mount(["/settings/profile"]);
    const nav = await screen.findByRole("navigation", { name: "Settings" });
    await userEvent.setup().type(within(nav).getByRole("searchbox", { name: "Find a setting" }), "security");
    const results = await within(nav).findByRole("list", { name: "Matching settings" });
    const item = within(results).getAllByRole("listitem")[0] as HTMLElement;
    expect(item).toHaveClass("max-lg:before:left-1.5");
    expect(item).not.toHaveClass("max-lg:before:left-[54px]");
  });
});
