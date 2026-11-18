// W-06 screens against the real link wired to the mock desktop (dev/mockLink.ts).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { registerPersistAdapter, MemoryPersistAdapter, type PersistAdapter } from "@/data";
import { getLinkConnection, resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { resetSessionActivity } from "./activityStore";
import { DESKTOP_ID, connectMockDesktop, emptyWorkspace, seedWorkspace, type MockLink, type MockLinkOptions } from "./dev/mockLink";
import { DRAWER_FILTER_KEY } from "./drawerFilter";
import { lastPlaceKey } from "./lastPlace";

/** A matchMedia stub: `wide` makes the 640 px query match. */
function stubMatchMedia(wide: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: wide && query.includes("min-width: 640px"),
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

beforeAll(() => {
  stubMatchMedia(false);
  // Radix menus and vaul use pointer capture, scrollIntoView and matchMedia, which jsdom lacks.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

let previous: PersistAdapter;
let link: MockLink;
beforeEach(() => {
  localStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
});
afterEach(() => {
  stubMatchMedia(false);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
  Object.defineProperty(window, "innerHeight", { value: 768, configurable: true, writable: true });
});

function mount(path: string, options: MockLinkOptions & { empty?: boolean } = {}) {
  link = connectMockDesktop({
    ...options,
    seed: (desktop) => {
      if (options.empty) emptyWorkspace(desktop);
      else seedWorkspace(desktop);
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient };
}

describe("catalog home", () => {
  it("lists projects, Monos and recent sessions of the desktop", async () => {
    mount("");
    expect(await screen.findByRole("heading", { level: 1, name: "Workspace" })).toBeInTheDocument();
    const projects = await screen.findByRole("region", { name: "Projects" });
    expect(within(projects).getByText("Example Alpha")).toBeInTheDocument();
    expect(within(projects).getByText("Gamma Service")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Monos" })).getByText("Example Mono")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Recent sessions" })).getAllByTestId("session-card").length).toBe(8);
  });

  it("draws no sidebar of its own at 640px and up: the app shell owns it (U-02)", async () => {
    stubMatchMedia(true);
    mount("");
    await screen.findByRole("heading", { level: 1, name: "Workspace" });
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(screen.queryByRole("navigation", { name: "Workspace" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Open workspace list/ })).toBeNull();
  });

  it("shows 'Make yourself at home' with a Choose projects link when nothing is shared (F-14)", async () => {
    mount("", { empty: true });
    expect(await screen.findByText("Make yourself at home.")).toBeInTheDocument();
    const choose = screen.getByRole("link", { name: "Choose projects" });
    expect(choose).toHaveAttribute("href", `/d/${DESKTOP_ID}/settings`);
  });

  it("remembers the place for `/` (F-14)", async () => {
    mount("/p/proj-alpha");
    await screen.findByRole("heading", { level: 1, name: "Example Alpha" });
    await waitFor(() => expect(localStorage.getItem(lastPlaceKey(DESKTOP_ID))).toBe(`/d/${DESKTOP_ID}/p/proj-alpha`));
    expect(localStorage.getItem("monoptah.lastDesktop")).toBe(DESKTOP_ID);
  });
});

describe("last used desktop (N-12)", () => {
  it("is remembered from a screen that is no workspace place, so `/` can bring the user back", async () => {
    mount("/settings");
    await waitFor(() => expect(localStorage.getItem("monoptah.lastDesktop")).toBe(DESKTOP_ID));
    expect(localStorage.getItem(lastPlaceKey(DESKTOP_ID))).toBeNull();
  });
});

describe("workspace drawer (F-12)", () => {
  async function openDrawer(user: ReturnType<typeof userEvent.setup>) {
    await screen.findByRole("heading", { level: 1, name: "Workspace" });
    await user.click(screen.getByRole("button", { name: /^Open workspace list/ }));
    return screen.findByRole("navigation", { name: "Workspace" });
  }

  it("lists Monos first, then projects", async () => {
    const user = userEvent.setup();
    mount("");
    const nav = await openDrawer(user);
    await within(nav).findByText("Example Alpha");
    const labels = within(nav).getAllByRole("heading", { level: 2 }).map((h) => h.textContent);
    expect(labels).toEqual(["Monos", "Projects"]);
    const names = within(nav).getAllByRole("link").map((link) => link.textContent ?? "");
    expect(names.findIndex((t) => t.includes("Research Mono"))).toBeLessThan(names.findIndex((t) => t.includes("Example Alpha")));
  });

  it("offers New Mono when the desktop advertises monoTools, and it opens the create dialog on the Mono list", async () => {
    const user = userEvent.setup();
    const { router } = mount("");
    const nav = await openDrawer(user);
    await within(nav).findByText("Example Alpha");
    await waitFor(() => expect(link.desktop.capabilities).toContain("monoTools"));
    await user.click(await within(nav).findByRole("button", { name: "New Mono" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/monos`));
    expect(await screen.findByTestId("create-mono")).toBeInTheDocument();
  });

  it("narrows by search and the Working now filter, and keeps the filter after a reload", async () => {
    const user = userEvent.setup();
    mount("");
    const nav = await openDrawer(user);
    await within(nav).findByText("Example Alpha");
    await user.type(within(nav).getByRole("searchbox", { name: "Search" }), "gamma");
    expect(within(nav).queryByText("Example Alpha")).toBeNull();
    expect(within(nav).getByText("Gamma Service")).toBeInTheDocument();
    await user.clear(within(nav).getByRole("searchbox", { name: "Search" }));

    await user.click(within(nav).getByRole("button", { name: "Filter" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Working now" }));
    expect(JSON.parse(localStorage.getItem(DRAWER_FILTER_KEY) ?? "{}")).toMatchObject({ filter: "working" });
    // Working now keeps the busy project and the working Mono only.
    expect(within(nav).getByText("Gamma Service")).toBeInTheDocument();
    expect(within(nav).queryByText("Example Alpha")).toBeNull();
    expect(within(nav).getByText("Research Mono")).toBeInTheDocument();
  });

  it("applies the stored filter on the next load", async () => {
    localStorage.setItem(DRAWER_FILTER_KEY, JSON.stringify({ filter: "unread", show: "monos" }));
    const user = userEvent.setup();
    mount("");
    const nav = await openDrawer(user);
    await within(nav).findByText("Example Mono");
    expect(within(nav).queryByText("Research Mono")).toBeNull();
    expect(within(nav).queryByRole("region", { name: "Projects" })).toBeNull();
    expect(within(nav).getByRole("button", { name: "Filter, on" })).toBeInTheDocument();
  });

  it("renders the desktop-nav-items slot", async () => {
    registerSlot("desktop-nav-items", (props) => <a href="/cloud-x">Cloud for {String(props.desktopId).slice(0, 4)}</a>);
    const user = userEvent.setup();
    mount("");
    const nav = await openDrawer(user);
    expect(within(nav).getByRole("link", { name: "Cloud for 1111" })).toBeInTheDocument();
  });
});

describe("project sessions list (F-18, F-21, F-24)", () => {
  it("shows the project's cards in desktop order with the working label, age, branch and model", async () => {
    mount("/p/proj-alpha");
    expect(await screen.findByRole("heading", { level: 1, name: "Example Alpha" })).toBeInTheDocument();
    const list = await screen.findByTestId("sessions-list");
    const cards = await within(list).findAllByTestId("session-card");
    const working = cards.find((card) => card.dataset.sessionId === "sess-seed-0")!;
    expect(within(working).getByText("Working...")).toBeInTheDocument();
    expect(within(working).getByText("main")).toBeInTheDocument();
    expect(within(working).getByText("Example Model One")).toBeInTheDocument();
    expect(within(working).getByRole("link")).toHaveAttribute("href", `/d/${DESKTOP_ID}/s/sess-seed-0`);
    // An idle session shows its age instead.
    const idle = cards.find((card) => card.dataset.sessionId === "sess-seed-5")!;
    expect(within(idle).getByText(/^\d+[mhd]$/)).toBeInTheDocument();
  });

  it("virtualizes the rows and pages the next 30 in when the window reaches the end", async () => {
    mount("/p/proj-alpha");
    const list = await screen.findByTestId("sessions-list");
    await within(list).findAllByTestId("session-card");
    // 768 px of window: far fewer rows than the first page of 30 are in the DOM.
    expect(within(list).getAllByTestId("session-card").length).toBeLessThan(25);
    const connection = getLinkConnection(DESKTOP_ID);
    const offsets: number[] = [];
    const original = connection.request.bind(connection);
    connection.request = ((method: string, params: { offset?: number }, ...rest: unknown[]) => {
      if (method === "sessions.list") offsets.push(params.offset ?? 0);
      return (original as (...args: unknown[]) => unknown)(method, params, ...rest);
    }) as typeof connection.request;
    // A tall window: every row is in range, so the last page is requested.
    Object.defineProperty(window, "innerHeight", { value: 6000, configurable: true, writable: true });
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    await waitFor(() => expect(offsets).toContain(30));
    await waitFor(() => expect(within(screen.getByTestId("sessions-list")).getAllByTestId("session-card").length).toBeGreaterThan(30));
  });

  it("searches on the desktop after a pause", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    await user.type(screen.getByRole("searchbox", { name: "Search sessions" }), "pagination");
    await waitFor(() => {
      const cards = screen.getAllByTestId("session-card");
      expect(cards.length).toBeGreaterThan(0);
      for (const card of cards) expect(card.textContent).toMatch(/pagination/i);
    });
    await user.clear(screen.getByRole("searchbox", { name: "Search sessions" }));
    await user.type(screen.getByRole("searchbox", { name: "Search sessions" }), "zzzz-nothing");
    expect(await screen.findByText("No matching sessions.")).toBeInTheDocument();
  });

  it("says when a project is not on this device", async () => {
    mount("/p/proj-missing");
    expect(await screen.findByText("This project is not on this device")).toBeInTheDocument();
  });

  it("session-card-badge slot renders on each card", async () => {
    registerSlot("session-card-badge", (props) => <span data-testid="badge">{(props.session as { id: string }).id.slice(-1)}</span>);
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    expect((await screen.findAllByTestId("badge")).length).toBeGreaterThan(3);
  });

  it("the card menu follows the session-capabilities predicate for the session's target", async () => {
    registerSessionCapabilities((capability, context) => context.target?.kind === "local" || context.target == null || capability === "files");
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    await screen.findAllByTestId("session-card");
    const find = (id: string) => screen.getAllByTestId("session-card").find((card) => card.dataset.sessionId === id)!;
    await user.click(within(find("sess-codespace-1")).getByRole("button", { name: /Actions for/ }));
    let menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Files" })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: "Git" })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: "Delete" })).toBeNull();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await user.click(within(find("sess-local-1")).getByRole("button", { name: /Actions for/ }));
    menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Git" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Delete" })).toBeInTheDocument();
  });
});

describe("now working pill (F-28)", () => {
  it("shows the sessions at work with their status and Stop sends session.cancel", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const pill = await screen.findByTestId("now-working");
    // sess-seed-0 works; sess-seed-1 works and has a question; sess-local-approval works and waits.
    expect(within(pill).getByRole("button", { name: /working sessions/ })).toBeInTheDocument();
    const before = [...link.desktop.world.sessions.values()].filter((s) => s.busy && s.projectId === "proj-alpha").length;
    expect(before).toBeGreaterThanOrEqual(2);
    await user.click(within(pill).getByRole("button", { name: "Stop" }));
    await waitFor(() => {
      const after = [...link.desktop.world.sessions.values()].filter((s) => s.busy && s.projectId === "proj-alpha").length;
      expect(after).toBe(before - 1);
    });
  });

  it("fans the sessions out and names a question and an approval", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const pill = await screen.findByTestId("now-working");
    await user.click(within(pill).getByRole("button", { name: /working sessions/ }));
    const rows = within(await screen.findByRole("list", { name: "Working sessions" })).getAllByRole("listitem");
    expect(rows.length).toBeGreaterThanOrEqual(2);
    expect(within(pill).getAllByText(/Has a question for you|Needs your approval/).length).toBeGreaterThan(0);
  });

  it("skips to the next session", async () => {
    const user = userEvent.setup();
    mount("/p/proj-alpha");
    const pill = await screen.findByTestId("now-working");
    const first = within(pill).getByText(/\d of \d/).textContent;
    await user.click(within(pill).getByRole("button", { name: "Skip to the next session" }));
    expect(within(pill).getByText(/\d of \d/).textContent).not.toBe(first);
  });
});

describe("read-only device", () => {
  it("shows the chip, disables Stop and New session", async () => {
    mount("/p/proj-alpha", { permission: "read" });
    expect(await screen.findByTestId("read-only-chip")).toHaveTextContent("Read only");
    const pill = await screen.findByTestId("now-working");
    expect(within(pill).getByRole("button", { name: "Stop" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "New session" })).toBeDisabled();
  });

  it("a control device has a live New session link and an enabled Stop", async () => {
    mount("/p/proj-alpha");
    expect(await screen.findByRole("link", { name: "New session" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/p/proj-alpha/new`);
    const pill = await screen.findByTestId("now-working");
    expect(within(pill).getByRole("button", { name: "Stop" })).toBeEnabled();
    expect(screen.queryByTestId("read-only-chip")).toBeNull();
  });
});
