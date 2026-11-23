// U-02: the app shell at 390 (passthrough), 768 (tablet) and 1280 (desktop), the right-panel host, the
// shortcuts and the command palette, against the real router and the mock desktop.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/features/backgrounds", () => ({
  BackgroundControls: ({ projectName }: { projectName: string }) => <p>Background controls of {projectName}</p>,
  BACKGROUND_CONTAINER_CLASS: "relative isolate",
  ChatBackground: () => null,
  useFirstBackgroundProject: () => null,
}));
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { DesktopSwitcher } from "@/features/desktops/DesktopSwitcher";
import { AppShell } from "./AppShell";
import { AppearanceSection } from "@/features/settings/AppearanceSection";
import { LAYOUT_MODE_KEY, PANEL_WIDTH_KEY, SESSION_LIST_WIDTH, SESSION_LIST_WIDTH_KEY, SIDEBAR_COLLAPSED_KEY, SIDEBAR_WIDTH_KEY, dockedPanelMax, reloadLayoutPrefs, useLayoutPrefs } from "./layoutPrefs";
import { closeCommandPalette, isCommandPaletteOpen, openCommandPalette } from "./palette";
import { clearPanels, closePanel, openPanel, registerPanel, usePanels } from "./panels";
import { clearCreateMonoRequest } from "@/features/monos/createRequest";
import { closeBackgroundDialog } from "./projectTools";
import { clearShortcuts, isMod, registerShortcut } from "./shortcuts";
import { resetTabsStore } from "./tabs/tabsStore";

const mediaListeners = new Set<() => void>();
let viewportWidth = 1280;

/** matchMedia for a viewport `width` px wide: every `(min-width: N px|rem)` query matches when width >= N (1rem = 16px). */
function stubViewport(width: number) {
  viewportWidth = width;
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  window.matchMedia = ((query: string) => {
    const found = /min-width:\s*([\d.]+)(px|rem)/.exec(query);
    const min = found ? [found[0], String(Number(found[1]) * (found[2] === "rem" ? 16 : 1))] : null;
    const listeners = new Map<EventListener, () => void>();
    return {
      get matches() {
        return min ? viewportWidth >= Number(min[1]) : false;
      },
      media: query,
      onchange: null,
      addEventListener: (_type: string, listener: EventListener) => {
        const wrapped = () => listener(new Event("change"));
        listeners.set(listener, wrapped);
        mediaListeners.add(wrapped);
      },
      removeEventListener: (_type: string, listener: EventListener) => {
        const wrapped = listeners.get(listener);
        if (wrapped) mediaListeners.delete(wrapped);
      },
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  }) as unknown as typeof window.matchMedia;
}

/** Changes the viewport of a mounted shell, like a window resize. */
function resizeViewport(width: number) {
  act(() => {
    stubViewport(width);
    window.dispatchEvent(new Event("resize"));
    for (const listener of mediaListeners) listener();
  });
}

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

// The route files import the panel registrations for their side effects: start every test from an empty registry,
// so `vitest -t` (which skips the earlier tests' cleanup) finds nothing registered twice.
beforeEach(() => {
  clearPanels();
  clearShortcuts();
});

let previous: PersistAdapter;
beforeEach(() => {
  localStorage.clear();
  resetTabsStore();
  reloadLayoutPrefs();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  registerSlot("app-shell", AppShell);
});
afterEach(() => {
  // Unmount before the viewport goes back to 1280: a still-mounted project screen would re-render in the split layout
  // (its 64rem query now matches) and open a link connection that leaks into the next test.
  cleanup();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  clearPanels();
  clearShortcuts();
  closeCommandPalette();
  closeBackgroundDialog();
  clearCreateMonoRequest();
  localStorage.clear();
  stubViewport(1280);
});

function mount(width: number, path = "/cloud", options: MockLinkOptions = {}) {
  stubViewport(width);
  connectMockDesktop({ ...options, seed: seedWorkspace });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, view };
}

const target = { desktopId: DESKTOP_ID, projectId: "proj-alpha" };
const local = { target: { kind: "local" }, desktopCapabilities: [] };
const ALPHA = "/p/proj-alpha";

function FilesPanel({ projectId }: { projectId: string }) {
  return <p>Files of {projectId}</p>;
}

describe("sidebar", () => {
  it("is passthrough at 390", async () => {
    mount(390);
    await screen.findByTestId("app-shell");
    expect(screen.getByTestId("app-shell")).toHaveAttribute("data-layout", "mobile");
    expect(screen.queryByTestId("app-sidebar")).toBeNull();
  });

  it.each(["", ALPHA, "/cloud"])("is the only sidebar on the home, project and cloud screens too (%s)", async (path) => {
    mount(1280, path);
    await screen.findByTestId("app-sidebar");
    expect(screen.getAllByRole("navigation", { name: "Workspace" })).toHaveLength(1);
    expect(screen.getAllByRole("complementary")).toHaveLength(1);
    expect(within(screen.getByTestId("app-sidebar")).getByRole("navigation", { name: "Workspace" })).toBeInTheDocument();
  });

  it.each([768, 1280])("has the activity rail beside the sidebar at %ipx, with Desktop settings and the avatar there and not in the sidebar", async (width) => {
    registerSlot("desktop-switcher", DesktopSwitcher);
    mount(width);
    const rail = await screen.findByTestId("activity-rail");
    const sidebar = await screen.findByTestId("app-sidebar");
    // The rail is the first column of the shell, right before the sidebar.
    expect(rail.nextElementSibling).toBe(sidebar);
    expect(within(rail).getByRole("link", { name: "Desktop settings" })).toHaveAttribute("href", `/settings/desktops/${DESKTOP_ID}`);
    expect(within(rail).getByRole("button", { name: "Account" })).toBeInTheDocument();
    expect(await within(rail).findByRole("link", { name: /current desktop/ })).toBeInTheDocument();
    // No duplicates in the sidebar.
    for (const name of ["All desktops", "Desktop settings", "Cloud"]) expect(within(sidebar).queryByRole("link", { name })).toBeNull();
    // The top bar is gone: no desktop switcher button and no Main nav.
    expect(screen.queryByRole("button", { name: /Switch desktop/ })).toBeNull();
    expect(screen.queryByRole("navigation", { name: "Main" })).toBeNull();
    // Search stays reachable in the sidebar.
    expect(within(sidebar).getByRole("button", { name: "Search" })).toBeInTheDocument();
  });

  it("keeps the rail next to the collapsed sidebar on a tablet", async () => {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, "1");
    reloadLayoutPrefs();
    mount(768);
    const sidebar = await screen.findByTestId("app-sidebar");
    expect(sidebar).toHaveAttribute("data-state", "collapsed");
    expect(screen.getByTestId("activity-rail").nextElementSibling).toBe(sidebar);
  });

  it("has no rail on a phone: the top bar keeps the switcher, Search and the avatar, and the drawer keeps its entries", async () => {
    registerSlot("desktop-switcher", DesktopSwitcher);
    mount(390);
    await screen.findByTestId("app-shell");
    expect(screen.queryByTestId("activity-rail")).toBeNull();
    expect(await screen.findByRole("button", { name: /Switch desktop/ })).toBeInTheDocument();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["Search", "Account"]);
  });

  it("has a Search button in the switcher row below 640px that opens the palette, and the rail takes over from 640px up", async () => {
    // The /d routes have no header: the phone Search button and the nav live in the desktop switcher row.
    registerSlot("desktop-switcher", DesktopSwitcher);
    const user = userEvent.setup();
    mount(390, ALPHA);
    await screen.findByTestId("app-shell");
    const header = screen.getByRole("navigation", { name: "Main" });
    await user.click(within(header).getByRole("button", { name: "Search" }));
    expect(isCommandPaletteOpen()).toBe(true);
    expect(await screen.findByTestId("command-palette")).toBeInTheDocument();
    closeCommandPalette();
    resizeViewport(768);
    // From 640px the activity rail replaces the top bar: no Main nav, no Search icon there (the sidebar has Search).
    await waitFor(() => expect(screen.queryByRole("navigation", { name: "Main" })).toBeNull());
    expect(screen.getByTestId("activity-rail")).toBeInTheDocument();
  });

  it("is a persistent 300px sidebar at 768 that collapses to a rail and remembers it", async () => {
    const user = userEvent.setup();
    const first = mount(768);
    const sidebar = await screen.findByTestId("app-sidebar");
    expect(sidebar).toHaveStyle({ width: "300px" });
    expect(within(sidebar).getByTestId("workspace-nav")).toBeInTheDocument();
    expect(screen.queryByRole("separator", { name: "Resize sidebar" })).toBeNull();

    await user.click(within(sidebar).getByRole("button", { name: "Collapse sidebar" }));
    expect(screen.getByTestId("app-sidebar")).toHaveAttribute("data-state", "collapsed");
    expect(localStorage.getItem(SIDEBAR_COLLAPSED_KEY)).toBe("1");
    expect(screen.queryByTestId("workspace-nav")).toBeNull();

    first.view.unmount();
    resetLinkConnections();
    clearLinkStores();
    reloadLayoutPrefs();
    mount(768);
    expect(await screen.findByRole("button", { name: "Expand sidebar" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Expand sidebar" }));
    expect(await screen.findByTestId("workspace-nav")).toBeInTheDocument();
    expect(localStorage.getItem(SIDEBAR_COLLAPSED_KEY)).toBe("0");
  });

  it("is resizable at 1280: the splitter clamps to 320-360 and remembers the width", async () => {
    const user = userEvent.setup();
    mount(1280);
    const sidebar = await screen.findByTestId("app-sidebar");
    expect(sidebar).toHaveStyle({ width: "320px" });
    expect(screen.queryByRole("button", { name: "Collapse sidebar" })).toBeNull();
    const splitter = screen.getByRole("separator", { name: "Resize sidebar" });
    splitter.focus();
    expect(splitter).toHaveAttribute("aria-valuemin", "320");
    expect(splitter).toHaveAttribute("aria-valuemax", "360");

    await user.keyboard("{ArrowRight}");
    expect(screen.getByTestId("app-sidebar")).toHaveStyle({ width: "336px" });
    await user.keyboard("{ArrowRight}{ArrowRight}{ArrowRight}");
    expect(screen.getByTestId("app-sidebar")).toHaveStyle({ width: "360px" });
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe("360");
    await user.keyboard("{Home}");
    expect(screen.getByTestId("app-sidebar")).toHaveStyle({ width: "320px" });
    await user.keyboard("{ArrowLeft}");
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe("320");
  });

  it("restores a stored width (clamped)", async () => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, "900");
    reloadLayoutPrefs();
    mount(1280);
    expect(await screen.findByTestId("app-sidebar")).toHaveStyle({ width: "360px" });
  });

  it("opens the palette from the sidebar button", async () => {
    const user = userEvent.setup();
    mount(1280);
    const sidebar = await screen.findByTestId("app-sidebar");
    await user.click(within(sidebar).getByRole("button", { name: "Search" }));
    expect(await screen.findByRole("combobox")).toBeInTheDocument();
  });
});

describe("classic layout: session list sidebar and chat pane (>= 1024)", () => {
  it("shows the project's session list between the sidebar and the chat pane on a project screen", async () => {
    mount(1280, ALPHA);
    const list = await screen.findByTestId("session-list-pane");
    const shell = screen.getByTestId("app-shell");
    expect(shell).toHaveAttribute("data-layout", "desktop");
    // Left sidebar, then the session list, then the chat pane, in that order.
    const order = [screen.getByTestId("app-sidebar"), list, screen.getByTestId("chat-pane")];
    expect(order.map((node) => node.parentElement)).toEqual([shell, shell, shell]);
    expect(order[0]!.compareDocumentPosition(order[1]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(order[1]!.compareDocumentPosition(order[2]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(list).toHaveStyle({ width: "320px" });
    expect(await within(list).findByRole("heading", { level: 1, name: "Example Alpha" })).toBeInTheDocument();
    expect(within(list).getByRole("searchbox", { name: "Search sessions" })).toBeInTheDocument();
    expect(await within(list).findByTestId("sessions-list")).toBeInTheDocument();
    // The chat already shows the New session page: no button that points at the page already open.
    expect(within(list).queryByRole("link", { name: "New session" })).toBeNull();
    // No project route heading twice: the chat pane (the New session page) has no title of its own.
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("shows the project's avatar, decorative, before the title in the session list header", async () => {
    mount(1280, ALPHA);
    const list = await screen.findByTestId("session-list-pane");
    const title = await within(list).findByRole("heading", { level: 1, name: "Example Alpha" });
    const avatar = await within(list).findByTestId("project-avatar");
    expect(avatar).toHaveAttribute("aria-hidden", "true");
    expect(avatar.querySelector("svg")).not.toBeNull();
    expect(avatar.compareDocumentPosition(title) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(list).queryByRole("button", { name: /^Workspace:/ })).toBeNull();
  });

  it("feeds the activity tracker from the session list, so the now-working pill shows where no workspace frame is mounted", async () => {
    mount(1280, "/s/sess-local-1");
    const list = await screen.findByTestId("session-list-pane");
    expect(await within(list).findByTestId("now-working")).toBeInTheDocument();
    expect(screen.queryByTestId("workspace-frame")).toBeNull();
  });

  it("shows no New session button on /new either: that page is already open", async () => {
    mount(1280, `${ALPHA}/new`);
    const list = await screen.findByTestId("session-list-pane");
    await within(list).findByRole("heading", { level: 1, name: "Example Alpha" });
    expect(within(list).queryByRole("link", { name: "New session" })).toBeNull();
  });

  it("fills the chat pane edge to edge with the New session page, with no card", async () => {
    mount(1700, ALPHA);
    const pane = await screen.findByTestId("chat-pane");
    const screenEl = await within(pane).findByTestId("new-session-screen");
    expect(screenEl).toHaveAttribute("data-embedded", "true");
    for (const node of [pane, screenEl]) {
      expect(node.className).not.toMatch(/rounded|border|max-w-/);
    }
    // The composer's own column keeps a readable width, centered.
    expect(within(pane).getByRole("textbox", { name: "Message" })).toBeInTheDocument();
  });

  it("keeps the session list on a session screen, for that session's project, with the chat beside it", async () => {
    mount(1280, "/s/sess-local-1");
    const list = await screen.findByTestId("session-list-pane");
    expect(await within(list).findByRole("heading", { level: 2, name: "Example Alpha" })).toBeInTheDocument();
    // On a session the New session page is not open, so the list offers it.
    expect(within(list).getByRole("link", { name: "New session" })).toHaveAttribute("href", expect.stringContaining("/p/proj-alpha/new"));
    const pane = screen.getByTestId("chat-pane");
    expect(await within(pane).findByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    // The session list already is the way back.
    expect(within(pane).getByRole("link", { name: "Back" }).className).toContain("lg:hidden");
  });

  it("has no session list on screens without a project, at tablet width and on a phone", async () => {
    mount(1280, "/cloud");
    await screen.findByTestId("app-sidebar");
    expect(screen.queryByTestId("session-list-pane")).toBeNull();
    resizeViewport(768);
    await waitFor(() => expect(screen.getByTestId("app-shell")).toHaveAttribute("data-layout", "tablet"));
    expect(screen.queryByTestId("session-list-pane")).toBeNull();
  });

  it("shows no session list below 1024 even on a project screen", async () => {
    mount(768, ALPHA);
    await screen.findByTestId("app-sidebar");
    expect(screen.queryByTestId("session-list-pane")).toBeNull();
    resizeViewport(390);
    await waitFor(() => expect(screen.getByTestId("app-shell")).toHaveAttribute("data-layout", "mobile"));
    expect(screen.queryByTestId("session-list-pane")).toBeNull();
    resizeViewport(1280);
    expect(await screen.findByTestId("session-list-pane")).toBeInTheDocument();
  });

  it("is resizable: the splitter clamps to 240-560 and remembers the width", async () => {
    const user = userEvent.setup();
    mount(1700, ALPHA);
    await screen.findByTestId("session-list-pane");
    const splitter = screen.getByRole("separator", { name: "Resize session list" });
    expect(splitter).toHaveAttribute("aria-valuemin", "240");
    expect(splitter).toHaveAttribute("aria-valuemax", "560");
    splitter.focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByTestId("session-list-pane")).toHaveStyle({ width: "336px" });
    expect(localStorage.getItem(SESSION_LIST_WIDTH_KEY)).toBe("336");
    await user.keyboard("{Home}");
    expect(screen.getByTestId("session-list-pane")).toHaveStyle({ width: "240px" });
    await user.keyboard("{End}");
    expect(screen.getByTestId("session-list-pane")).toHaveStyle({ width: "560px" });
  });

  it("restores a stored width, clamped, and keeps 320px for the chat", async () => {
    localStorage.setItem(SESSION_LIST_WIDTH_KEY, "900");
    reloadLayoutPrefs();
    expect(useLayoutPrefs.getState().sessionListWidth).toBe(560);
    mount(1100, ALPHA);
    // 1100 - 320 (sidebar) - 320 (chat) = 460.
    expect(await screen.findByTestId("session-list-pane")).toHaveStyle({ width: "460px" });
  });
});

describe("layout mode: Classic and Focused", () => {
  it("is Classic by default and keeps the session list", async () => {
    mount(1280, ALPHA);
    expect(await screen.findByTestId("session-list-pane")).toBeInTheDocument();
    expect(useLayoutPrefs.getState().layoutMode).toBe("classic");
  });

  it("Focused leaves the session list column out and shows the project page in the chat pane", async () => {
    localStorage.setItem(LAYOUT_MODE_KEY, "focused");
    reloadLayoutPrefs();
    mount(1280, ALPHA);
    await screen.findByTestId("app-sidebar");
    expect(await screen.findByTestId("workspace-frame")).toBeInTheDocument();
    expect(screen.queryByTestId("session-list-pane")).toBeNull();
    expect(screen.queryByTestId("chat-new-session")).toBeNull();
  });

  it("an unknown stored value is Classic", () => {
    localStorage.setItem(LAYOUT_MODE_KEY, "sideways");
    reloadLayoutPrefs();
    expect(useLayoutPrefs.getState().layoutMode).toBe("classic");
  });

  it("switches at once from Settings > Appearance and remembers the choice in this browser", async () => {
    const user = userEvent.setup();
    const { router } = mount(1280, ALPHA);
    expect(await screen.findByTestId("session-list-pane")).toBeInTheDocument();
    // The picker is the one of the Appearance card; it works on the live shell.
    const picker = render(<AppearanceSection />);
    const group = within(picker.container).getByRole("radiogroup", { name: "Layout" });
    expect(within(group).getByRole("radio", { name: "Classic" })).toBeChecked();
    await user.click(within(group).getByRole("radio", { name: "Focused" }));
    expect(localStorage.getItem(LAYOUT_MODE_KEY)).toBe("focused");
    await waitFor(() => expect(screen.queryByTestId("session-list-pane")).toBeNull());
    await user.click(within(group).getByRole("radio", { name: "Classic" }));
    expect(localStorage.getItem(LAYOUT_MODE_KEY)).toBe("classic");
    expect(await screen.findByTestId("session-list-pane")).toBeInTheDocument();
    expect(router).toBeDefined();
  });

  it("disables the picker on a tablet (no session-list column there) and hides it on a phone", () => {
    stubViewport(768);
    const tablet = render(<AppearanceSection />);
    expect(within(tablet.container).getByRole("radio", { name: "Focused" })).toBeDisabled();
    expect(within(tablet.container).getByRole("radiogroup", { name: "Layout" })).toHaveAttribute("aria-disabled", "true");
    tablet.unmount();
    stubViewport(390);
    const phone = render(<AppearanceSection />);
    expect(within(phone.container).queryByRole("radiogroup", { name: "Layout" })).toBeNull();
    expect(within(phone.container).getByRole("radiogroup", { name: "Colour scheme" })).toBeInTheDocument();
  });
});

describe("right panel host", () => {
  beforeEach(() => registerPanel("files", { title: "Files", component: FilesPanel }));

  it("is a sheet at 390", async () => {
    mount(390, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    const sheet = await screen.findByRole("dialog", { name: "Files" });
    expect(within(sheet).getByText("Files of proj-alpha")).toBeInTheDocument();
    expect(screen.queryByTestId("panel-overlay")).toBeNull();
  });

  it("is a 360px overlay at 768 that Esc dismisses", async () => {
    const user = userEvent.setup();
    mount(768, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    const overlay = await screen.findByTestId("panel-overlay");
    expect(overlay).toHaveStyle({ width: "360px" });
    expect(screen.queryByRole("separator", { name: "Resize panel" })).toBeNull();
    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("panel-overlay")).toBeNull();
    expect(usePanels.getState().open).toBeNull();
  });

  it("is docked and resizable at 1600, clamped to 380-480", async () => {
    const user = userEvent.setup();
    mount(1600, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    const docked = await screen.findByTestId("panel-docked");
    expect(docked).toHaveStyle({ width: "420px" });
    const splitter = screen.getByRole("separator", { name: "Resize panel" });
    splitter.focus();
    // The panel is to the right of its splitter: ArrowLeft grows it.
    await user.keyboard("{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}{ArrowLeft}");
    expect(screen.getByTestId("panel-docked")).toHaveStyle({ width: "480px" });
    expect(localStorage.getItem(PANEL_WIDTH_KEY)).toBe("480");
    await user.keyboard("{Home}");
    expect(screen.getByTestId("panel-docked")).toHaveStyle({ width: "380px" });
    // Docked beside the main pane: both are on screen.
    expect(screen.getByTestId("app-shell")).toContainElement(docked);
    await user.click(screen.getByRole("button", { name: "Close panel" }));
    expect(screen.queryByTestId("panel-docked")).toBeNull();
  });

  it("does not close the panel on Esc inside another dialog", async () => {
    mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByTestId("panel-docked");
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    const input = document.createElement("input");
    dialog.appendChild(input);
    document.body.appendChild(dialog);
    fireEvent.keyDown(input, { key: "Escape" });
    expect(usePanels.getState().open).not.toBeNull();
    dialog.remove();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(usePanels.getState().open).toBeNull();
  });
});

describe("right panel host, per project", () => {
  beforeEach(() => registerPanel("files", { title: "Files", component: FilesPanel }));

  const go = (router: ReturnType<typeof mount>["router"], path: string) => act(() => router.history.push(`/d/${DESKTOP_ID}${path}`));

  it("shows a panel only on the screens of its project", async () => {
    const { router } = mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    expect(await screen.findByTestId("panel-docked")).toBeInTheDocument();

    go(router, "/p/proj-beta");
    await waitFor(() => expect(screen.queryByTestId("panel-docked")).toBeNull());
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(usePanels.getState().open).not.toBeNull(); // Esc must not close a panel that is not on screen

    go(router, "");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}`));
    expect(screen.queryByTestId("panel-docked")).toBeNull();

    go(router, "/s/sess-sandbox-stopped"); // a session of proj-beta
    await waitFor(() => expect(router.state.location.pathname).toContain("/s/sess-sandbox-stopped"));
    expect(screen.queryByTestId("panel-docked")).toBeNull();

    go(router, "/s/sess-local-1"); // a session of proj-alpha
    expect(await screen.findByTestId("panel-docked")).toBeInTheDocument();
    go(router, ALPHA);
    expect(await screen.findByTestId("panel-docked")).toBeInTheDocument();
  });

  it("Esc closes the panel of the current project", async () => {
    mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByTestId("panel-docked");
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(usePanels.getState().open).toBeNull();
  });

  it("follows the viewport: docked at 1280, overlay at 768, sheet at 390, docked again", async () => {
    mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByTestId("panel-docked");
    resizeViewport(768);
    expect(await screen.findByTestId("panel-overlay")).toBeInTheDocument();
    expect(screen.queryByTestId("panel-docked")).toBeNull();
    resizeViewport(390);
    expect(await screen.findByRole("dialog", { name: "Files" })).toBeInTheDocument();
    expect(screen.queryByTestId("panel-overlay")).toBeNull();
    resizeViewport(1280);
    expect(await screen.findByTestId("panel-docked")).toBeInTheDocument();
    expect(usePanels.getState().open?.kind).toBe("files");
  });

  it("keeps 320px for the chat pane: narrows the docked panel, falls back to the overlay when 380 does not fit", async () => {
    expect(dockedPanelMax(1024, 320)).toBe(384);
    expect(dockedPanelMax(1024, 360)).toBeNull();
    expect(dockedPanelMax(1100, 320)).toBe(460);
    expect(dockedPanelMax(1280, 320)).toBe(480);
    expect(dockedPanelMax(1280, 360)).toBe(480);
    // A project's screen also has the session list (240 at its narrowest): 320 + 240 + 380 + 320 = 1260.
    expect(dockedPanelMax(1259, 320 + SESSION_LIST_WIDTH.min)).toBeNull();
    expect(dockedPanelMax(1280, 320 + SESSION_LIST_WIDTH.min)).toBe(400);

    mount(1100, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    expect(await screen.findByTestId("panel-overlay")).toBeInTheDocument();
    expect(screen.queryByTestId("panel-docked")).toBeNull();
  });

  it("docks the panel at 1280 on a project screen by shrinking the session list to its minimum", async () => {
    mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    expect(screen.getByTestId("session-list-pane")).toHaveStyle({ width: "320px" });
    act(() => void openPanel("files", target, local));
    expect(await screen.findByTestId("panel-docked")).toHaveStyle({ width: "400px" });
    expect(screen.getByTestId("session-list-pane")).toHaveStyle({ width: "240px" });
    expect(screen.getByRole("separator", { name: "Resize panel" })).toHaveAttribute("aria-valuemax", "400");
    // The stored preference is untouched: the list comes back when the panel closes.
    expect(useLayoutPrefs.getState().sessionListWidth).toBe(320);
    act(() => closePanel());
    await waitFor(() => expect(screen.getByTestId("session-list-pane")).toHaveStyle({ width: "320px" }));
  });

  it("opens as the overlay at 1024 when a 360px sidebar leaves too little room", async () => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, "360");
    reloadLayoutPrefs();
    mount(1024, "/s/sess-local-1");
    await screen.findByTestId("app-sidebar");
    act(() => void openPanel("files", target, local));
    expect(await screen.findByTestId("panel-overlay")).toBeInTheDocument();
    expect(screen.queryByTestId("panel-docked")).toBeNull();
  });

  describe("overlay at 768", () => {
    it("moves focus into the panel, dismisses on a pointerdown outside and returns focus to the opener", async () => {
      mount(768, ALPHA);
      await screen.findByTestId("app-shell");
      const opener = document.createElement("button");
      document.body.appendChild(opener);
      opener.focus();
      act(() => void openPanel("files", target, local));
      const overlay = await screen.findByTestId("panel-overlay");
      await waitFor(() => expect(overlay.querySelector("[data-panel-heading]")).toHaveFocus());

      fireEvent.pointerDown(overlay); // inside: stays
      expect(usePanels.getState().open).not.toBeNull();
      fireEvent.pointerDown(document.body);
      expect(usePanels.getState().open).toBeNull();
      await waitFor(() => expect(opener).toHaveFocus());
      opener.remove();
    });

    it("lets a data-panel-trigger element toggle the panel without the outside dismissal racing it", async () => {
      mount(768, ALPHA);
      await screen.findByTestId("app-shell");
      const trigger = document.createElement("button");
      trigger.setAttribute("data-panel-trigger", "");
      document.body.appendChild(trigger);
      act(() => void openPanel("files", target, local));
      await screen.findByTestId("panel-overlay");
      fireEvent.pointerDown(trigger);
      expect(usePanels.getState().open).not.toBeNull();
      trigger.remove();
    });
  });
});

describe("persisted layout", () => {
  it("falls back to the defaults for bad stored values", () => {
    for (const bad of ["abc", "", "-5", "NaN", "1e3", "Infinity"]) {
      localStorage.setItem(SIDEBAR_WIDTH_KEY, bad);
      localStorage.setItem(PANEL_WIDTH_KEY, bad);
      localStorage.setItem(SESSION_LIST_WIDTH_KEY, bad);
      localStorage.setItem(SIDEBAR_COLLAPSED_KEY, bad);
      reloadLayoutPrefs();
      const state = useLayoutPrefs.getState();
      expect(state.sidebarWidth, bad).toBe(320);
      expect(state.panelWidth, bad).toBe(420);
      expect(state.sessionListWidth, bad).toBe(320);
      expect(state.sidebarCollapsed, bad).toBe(false);
    }
  });

  it("clamps out-of-range numbers", () => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, "10");
    localStorage.setItem(PANEL_WIDTH_KEY, "9000");
    localStorage.setItem(SESSION_LIST_WIDTH_KEY, "10");
    reloadLayoutPrefs();
    expect(useLayoutPrefs.getState().sessionListWidth).toBe(240);
    expect(useLayoutPrefs.getState().sidebarWidth).toBe(320);
    expect(useLayoutPrefs.getState().panelWidth).toBe(480);
  });

  it("writes a dragged width once, on pointer up, and draws one edge line", async () => {
    mount(1280);
    const sidebar = await screen.findByTestId("app-sidebar");
    expect(sidebar.className).not.toContain("border-r"); // the splitter draws the only stroke
    const splitter = screen.getByRole("separator", { name: "Resize sidebar" });
    fireEvent.pointerDown(splitter, { clientX: 100, pointerId: 1 });
    fireEvent.pointerMove(splitter, { clientX: 116, pointerId: 1 });
    fireEvent.pointerMove(splitter, { clientX: 130, pointerId: 1 });
    expect(screen.getByTestId("app-sidebar")).toHaveStyle({ width: "350px" });
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBeNull();
    fireEvent.pointerUp(splitter, { pointerId: 1 });
    expect(localStorage.getItem(SIDEBAR_WIDTH_KEY)).toBe("350");
  });

  it("the docked panel has no border of its own beside its splitter", async () => {
    mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    registerPanel("files", { title: "Files", component: FilesPanel });
    act(() => void openPanel("files", target, local));
    expect((await screen.findByTestId("panel-docked")).className).not.toContain("border-l");
  });
});

describe("macOS", () => {
  beforeEach(() => Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" }));
  afterEach(() => {
    delete (navigator as { platform?: string }).platform;
  });

  it("isMod is Cmd, not Ctrl", () => {
    expect(isMod({ metaKey: true, ctrlKey: false })).toBe(true);
    expect(isMod({ metaKey: false, ctrlKey: true })).toBe(false);
    expect(isMod({ metaKey: true, ctrlKey: true })).toBe(false);
  });

  it("Meta+K opens the palette, Ctrl+K is ignored, and the hint reads the same key", async () => {
    const user = userEvent.setup();
    mount(1280);
    const sidebar = await screen.findByTestId("app-sidebar");
    expect(within(sidebar).getByText("⌘K")).toBeInTheDocument();
    const ctrl = new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(ctrl);
    expect(ctrl.defaultPrevented).toBe(false);
    expect(isCommandPaletteOpen()).toBe(false);
    await user.keyboard("{Meta>}k{/Meta}");
    expect(await screen.findByRole("combobox")).toBeInTheDocument();
  });
});

describe("shortcut hint", () => {
  it("reads Ctrl K off macOS", async () => {
    mount(1280);
    expect(within(await screen.findByTestId("app-sidebar")).getByText("Ctrl K")).toBeInTheDocument();
  });
});

describe("shortcuts do not stack overlays", () => {
  function press(init: KeyboardEventInit) {
    const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
    window.dispatchEvent(event);
    return event;
  }

  it("Mod+K and Mod+P do nothing while another dialog is open, but the browser does not get the key", async () => {
    mount(1280);
    await screen.findByTestId("app-sidebar");
    const handler = vi.fn();
    registerShortcut("quick-open", handler);
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.appendChild(dialog);
    // Taken, so no address bar or print dialog opens over the app's dialog.
    expect(press({ key: "k", ctrlKey: true }).defaultPrevented).toBe(true);
    expect(press({ key: "p", ctrlKey: true }).defaultPrevented).toBe(true);
    expect(isCommandPaletteOpen()).toBe(false);
    expect(handler).not.toHaveBeenCalled();
    dialog.remove();
    press({ key: "k", ctrlKey: true });
    expect(isCommandPaletteOpen()).toBe(true);
  });

  it("Mod+P closes the palette before it runs quick-open", async () => {
    mount(1280);
    await screen.findByTestId("app-sidebar");
    const handler = vi.fn(() => expect(isCommandPaletteOpen()).toBe(false));
    registerShortcut("quick-open", handler);
    act(() => openCommandPalette());
    await screen.findByRole("combobox");
    press({ key: "p", ctrlKey: true });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(isCommandPaletteOpen()).toBe(false);
  });

  it("falls back to the physical key on a non-Latin layout", async () => {
    mount(1280);
    await screen.findByTestId("app-sidebar");
    const handler = vi.fn();
    registerShortcut("quick-open", handler);
    press({ key: "п", code: "KeyP", ctrlKey: true });
    expect(handler).toHaveBeenCalledTimes(1);
    press({ key: "л", code: "KeyK", ctrlKey: true });
    expect(isCommandPaletteOpen()).toBe(true);
  });
});

describe("shortcuts and palette", () => {
  it("Mod+K opens the palette, typing narrows, Enter navigates", async () => {
    const user = userEvent.setup();
    const { router } = mount(1280);
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    const box = await screen.findByRole("combobox");
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(2));
    await user.type(box, "gamma");
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
    expect(screen.getByRole("option")).toHaveTextContent("Gamma Service");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toMatch(new RegExp(`^/d/${DESKTOP_ID}/p/`)));
    await waitFor(() => expect(screen.queryByRole("combobox")).toBeNull());
  });

  it("shows a Settings group only once something is typed, and Enter opens the page and scrolls to the item", async () => {
    const user = userEvent.setup();
    const { router } = mount(1280);
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    const box = await screen.findByRole("combobox");
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(2));
    expect(screen.queryByRole("group", { name: "Settings" })).toBeNull();
    await user.type(box, "reconnect");
    const group = await screen.findByRole("group", { name: "Settings" });
    await user.click(within(group).getByRole("option", { name: /Reconnect/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/settings/desktops/${DESKTOP_ID}`));
    expect(router.state.location.hash).toBe("desktop-troubleshoot");
  });

  it("arrow keys move the selection and Enter opens a Mono", async () => {
    const user = userEvent.setup();
    const { router } = mount(1280);
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    const box = await screen.findByRole("combobox");
    await user.type(box, "research");
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(0));
    const first = screen.getAllByRole("option")[0]!;
    expect(first).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowDown}{ArrowUp}{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toMatch(/\/monos\//));
  });

  it("offers Monos and New Mono, and New Mono opens the create dialog on the Mono list (F-13, F-75)", async () => {
    const user = userEvent.setup();
    const { router } = mount(1280);
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    const box = await screen.findByRole("combobox");
    await user.type(box, "new mono");
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(1));
    expect(screen.getByRole("option")).toHaveTextContent("New Mono");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/monos`));
    expect(await screen.findByTestId("create-mono")).toBeInTheDocument();
  });

  it("opens the Mono list from the Monos action without opening the dialog", async () => {
    const user = userEvent.setup();
    const { router } = mount(1280);
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    await user.type(await screen.findByRole("combobox"), "monos");
    const goTo = await screen.findByRole("group", { name: "Go to" });
    await user.click(within(goTo).getByRole("option", { name: "Monos" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/monos`));
    await screen.findByTestId("mono-home");
    expect(screen.queryByTestId("create-mono")).toBeNull();
  });

  it("does not offer New Mono to a read-only device, or when the desktop has no Mono tools", async () => {
    const user = userEvent.setup();
    mount(1280, "/cloud", { permission: "read" });
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    await user.type(await screen.findByRole("combobox"), "mono");
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(0));
    expect(screen.queryByRole("option", { name: "New Mono" })).toBeNull();
    expect(screen.getByRole("option", { name: "Monos", description: "" })).toBeInTheDocument();
  });

  it("Esc closes the palette, and a second Mod+K closes it too", async () => {
    const user = userEvent.setup();
    mount(768);
    await screen.findByTestId("app-sidebar");
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByRole("combobox");
    await user.keyboard("{Control>}k{/Control}");
    await waitFor(() => expect(screen.queryByRole("combobox")).toBeNull());
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByRole("combobox");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("combobox")).toBeNull());
  });

  it("Mod+P calls the quick-open handler and is a no-op without one", async () => {
    const user = userEvent.setup();
    mount(1280);
    await screen.findByTestId("app-sidebar");
    const quiet = new KeyboardEvent("keydown", { key: "p", ctrlKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(quiet);
    expect(quiet.defaultPrevented).toBe(false);

    const handler = vi.fn();
    registerShortcut("quick-open", handler);
    await user.keyboard("{Control>}p{/Control}");
    expect(handler).toHaveBeenCalledTimes(1);
    const loud = new KeyboardEvent("keydown", { key: "p", ctrlKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(loud);
    expect(loud.defaultPrevented).toBe(true);
  });

  it("ignores keys while an IME composes", async () => {
    mount(1280);
    await screen.findByTestId("app-sidebar");
    const handler = vi.fn();
    registerShortcut("quick-open", handler);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "p", ctrlKey: true, isComposing: true, bubbles: true, cancelable: true }));
    expect(handler).not.toHaveBeenCalled();
  });

  it("registering a shortcut twice throws", () => {
    registerShortcut("quick-open", () => {});
    expect(() => registerShortcut("quick-open", () => {})).toThrow(/already registered/);
  });
});

describe("panel registry", () => {
  it("throws on a second registration of a kind", () => {
    registerPanel("git", { title: "Git", component: FilesPanel });
    expect(() => registerPanel("git", { title: "Git", component: FilesPanel })).toThrow(/already registered/);
  });

  it("cannot open an unregistered kind", () => {
    expect(openPanel("changes", target, local)).toBe(false);
    expect(usePanels.getState().open).toBeNull();
  });

  it("consults session-capabilities for the kind", () => {
    registerPanel("files", { title: "Files", component: FilesPanel });
    registerSessionCapabilities((_capability, context) => context.target?.kind === "local");
    expect(openPanel("files", target, { target: { kind: "cloud" }, desktopCapabilities: [] })).toBe(false);
    expect(openPanel("files", target, local)).toBe(true);
    closePanel();
  });
});

describe("Mod+K and Mod+P over the Files sheet", () => {
  function press(init: KeyboardEventInit) {
    const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
    window.dispatchEvent(event);
    return event;
  }

  it("work below 640px while the right-panel sheet is open", async () => {
    registerPanel("files", { title: "Files", component: FilesPanel });
    mount(390, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByRole("dialog", { name: "Files" });
    const handler = vi.fn();
    registerShortcut("quick-open", handler);
    expect(press({ key: "p", ctrlKey: true }).defaultPrevented).toBe(true);
    expect(handler).toHaveBeenCalledTimes(1);
    expect(press({ key: "k", ctrlKey: true }).defaultPrevented).toBe(true);
    expect(isCommandPaletteOpen()).toBe(true);
  });
});

describe("right panel overlay and other layers", () => {
  beforeEach(() => registerPanel("files", { title: "Files", component: FilesPanel }));

  it("stays open when the scrim of a dialog is pressed at 768", async () => {
    mount(768, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByTestId("panel-overlay");
    act(() => openCommandPalette());
    await screen.findByRole("combobox");
    const scrim = document.querySelector('[data-slot="dialog-overlay"]');
    expect(scrim).not.toBeNull();
    fireEvent.pointerDown(scrim!);
    expect(usePanels.getState().open).not.toBeNull();
    expect(screen.getByTestId("panel-overlay")).toBeInTheDocument();
  });

  it("still closes on a press outside when no layer is open", async () => {
    mount(768, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByTestId("panel-overlay");
    fireEvent.pointerDown(document.body);
    expect(usePanels.getState().open).toBeNull();
  });
});

const localOnly = (capability: string, context: { target?: { kind: string } | null }) =>
  !["files", "changes", "git"].includes(capability) || !context.target || context.target.kind === "local";

describe("project tools (project bar and palette)", () => {
  beforeEach(() => {
    for (const [kind, title] of [["files", "Files"], ["changes", "Changes"], ["git", "Git"]] as const) {
      registerPanel(kind, { title, component: () => <p>{title} body</p> });
    }
  });

  it("toggles Files, Changes and Git for the project, with trigger buttons that stay out of the outside-press rule", async () => {
    const user = userEvent.setup();
    mount(1280, ALPHA);
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    const files = within(bar).getByRole("button", { name: "Files" });
    for (const name of ["Files", "Changes", "Git"]) expect(within(bar).getByRole("button", { name })).toHaveAttribute("data-panel-trigger");
    expect(files).toHaveAttribute("aria-pressed", "false");
    await user.click(files);
    expect(await screen.findByTestId("right-panel")).toHaveAttribute("data-kind", "files");
    expect(files).toHaveAttribute("aria-pressed", "true");
    await user.click(within(bar).getByRole("button", { name: "Git" }));
    expect(screen.getByTestId("right-panel")).toHaveAttribute("data-kind", "git");
    await user.click(within(bar).getByRole("button", { name: "Git" }));
    expect(screen.queryByTestId("right-panel")).toBeNull();
  });

  it("shows the tool of the current route as current and never docks a second copy of it", async () => {
    const user = userEvent.setup();
    mount(1280, `${ALPHA}/files`);
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    const files = within(bar).getByRole("button", { name: "Files" });
    expect(files).toHaveAttribute("aria-current", "page");
    expect(files).not.toHaveAttribute("aria-pressed");
    expect(within(bar).getByRole("button", { name: "Changes" })).not.toHaveAttribute("aria-current");
    await user.click(files);
    expect(screen.queryByTestId("right-panel")).toBeNull();
    await user.click(within(bar).getByRole("button", { name: "Git" }));
    expect(await screen.findByTestId("right-panel")).toHaveAttribute("data-kind", "git");
  });

  it("toggles the overlay at 768 without the outside press closing and reopening it", async () => {
    const user = userEvent.setup();
    mount(768, ALPHA);
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    await user.click(within(bar).getByRole("button", { name: "Changes" }));
    await screen.findByTestId("panel-overlay");
    await user.click(within(bar).getByRole("button", { name: "Changes" }));
    expect(screen.queryByTestId("panel-overlay")).toBeNull();
  });

  it("is the icon row on its own row above the session search row below 640px in a project, and absent outside a project", async () => {
    mount(390, ALPHA);
    await screen.findByTestId("app-shell");
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    expect(bar).toHaveAttribute("data-variant", "inline");
    // The tools have their own row above the search row, which keeps the search box and the session filter.
    const search = screen.getByRole("searchbox", { name: "Search sessions" });
    expect(search.parentElement).not.toContainElement(bar);
    expect(bar.compareDocumentPosition(search) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(search.parentElement).toContainElement(screen.getByRole("button", { name: "Filter sessions" }));
    expect(screen.queryByRole("button", { name: /^Workspace:/ })).toBeNull();
    expect(screen.queryByTestId("app-sidebar")).toBeNull();
    expect(within(bar).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["Files", "Changes", "Git", "Background"]);
  });

  it("is absent below 640px on a screen without a project", async () => {
    mount(390, "/cloud");
    await screen.findByTestId("app-shell");
    expect(screen.queryByRole("toolbar", { name: "Project tools" })).toBeNull();
  });

  it("is absent on a screen without a project", async () => {
    mount(1280, "/cloud");
    await screen.findByTestId("app-shell");
    expect(screen.queryByRole("toolbar", { name: "Project tools" })).toBeNull();
  });

  it("follows the session's project and hides what the predicate refuses for a cloud session", async () => {
    registerSessionCapabilities(localOnly);
    const { router } = mount(1280, "/s/sess-local-1");
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    expect(within(bar).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["Files", "Changes", "Git", "Background"]);
    await act(async () => router.history.push(`/d/${DESKTOP_ID}/s/sess-codespace-1`));
    await waitFor(() => expect(within(screen.getByRole("toolbar", { name: "Project tools" })).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["Background"]));
  });

  it.each([768, 1280])("adds no element above the screen on a session route at %i, the tools sit in the sidebar", async (width) => {
    const { view } = mount(width, "/s/sess-local-1");
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    expect(screen.getByTestId("app-sidebar")).toContainElement(bar);
    const pane = view.container.querySelector("[data-testid='app-shell'] > .\\@container");
    expect(pane).not.toBeNull();
    expect(pane!.querySelector('[data-testid="project-bar"]')).toBeNull();
    // The tab strip is the one new element above the screen (an intended part of the chat pane); the screen is first in the pane body.
    expect(within(pane as HTMLElement).getByTestId("pane-body").firstElementChild).toHaveAttribute("data-testid", "route-stub");
  });

  it("keeps the tools in the sidebar rail as icon buttons when it is collapsed", async () => {
    const user = userEvent.setup();
    mount(768, "/s/sess-local-1");
    await user.click(await screen.findByRole("button", { name: "Collapse sidebar" }));
    const bar = await screen.findByRole("toolbar", { name: "Project tools" });
    expect(screen.getByTestId("app-sidebar")).toContainElement(bar);
    expect(within(bar).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(["Files", "Changes", "Git", "Background"]);
  });

  it("hides a panel opened on the local project while a cloud session of it is shown", async () => {
    registerSessionCapabilities(localOnly);
    const { router } = mount(1280, "/s/sess-local-1");
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    await screen.findByTestId("right-panel");
    await act(async () => router.history.push(`/d/${DESKTOP_ID}/s/sess-codespace-1`));
    await waitFor(() => expect(screen.queryByTestId("right-panel")).toBeNull());
  });

  it("opens the background picker for the project in a dialog", async () => {
    const user = userEvent.setup();
    mount(1280, ALPHA);
    await user.click(await screen.findByRole("button", { name: "Background" }));
    const dialog = await screen.findByRole("dialog", { name: "Background" });
    await waitFor(() => expect(within(dialog).getByText("Background controls of Example Alpha")).toBeInTheDocument());
  });

  it("offers the same actions in the command palette and opens a panel from it", async () => {
    const user = userEvent.setup();
    mount(1280, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => openCommandPalette());
    const box = await screen.findByRole("combobox");
    await waitFor(() => expect(screen.getAllByRole("group", { name: "This project" })).toHaveLength(1));
    const group = screen.getByRole("group", { name: "This project" });
    expect(within(group).getAllByRole("option").map((option) => option.textContent)).toEqual(["Files", "Changes", "Git", "Background"]);
    await user.type(box, "changes");
    await user.keyboard("{Enter}");
    expect(await screen.findByTestId("right-panel")).toHaveAttribute("data-kind", "changes");
  });

  it("opens a panel from the palette as the sheet below 640px", async () => {
    const user = userEvent.setup();
    mount(390, ALPHA);
    await screen.findByTestId("app-shell");
    act(() => openCommandPalette());
    await screen.findByRole("combobox");
    await user.click(await screen.findByRole("option", { name: "Git" }));
    expect(await screen.findByRole("dialog", { name: "Git" })).toBeInTheDocument();
  });

  it("offers no panel action for a refused session in the palette", async () => {
    registerSessionCapabilities(localOnly);
    mount(1280, "/s/sess-codespace-1");
    await screen.findByTestId("app-shell");
    act(() => openCommandPalette());
    await screen.findByRole("combobox");
    const group = await screen.findByRole("group", { name: "This project" });
    expect(within(group).getAllByRole("option").map((option) => option.textContent)).toEqual(["Background"]);
  });
});
