// Session tabs and split panes in the app shell (design-system.md section 11), against the real router and the mock
// desktop. The viewport helpers mirror layout.test.tsx.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, createEvent, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/features/backgrounds", () => ({
  BackgroundControls: () => null,
  BACKGROUND_CONTAINER_CLASS: "relative isolate",
  ChatBackground: () => null,
  useFirstBackgroundProject: () => null,
}));
import { readChatDraft, resetChatDrafts } from "@/features/composer/chatDraft";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { clearHooks } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { clearCreateMonoRequest } from "@/features/monos/createRequest";
import { AppShell } from "../AppShell";
import { reloadLayoutPrefs } from "../layoutPrefs";
import { closeCommandPalette } from "../palette";
import { clearPanels } from "../panels";
import { closeBackgroundDialog } from "../projectTools";
import { clearShortcuts } from "../shortcuts";
import { activateTab, emptyWorkspace, moveTab, openTab, paneOrder, serializeWorkspace, sessionTab, splitPane } from "./paneTree";
import { endTabDrag } from "./tabDrag";
import { getWorkspace, resetTabsStore, tabsKey } from "./tabsStore";

const mediaListeners = new Set<() => void>();
let viewportWidth = 1280;

function stubViewport(width: number) {
  viewportWidth = width;
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  window.matchMedia = ((query: string) => {
    const found = /min-width:\s*([\d.]+)(px|rem)/.exec(query);
    const min = found ? Number(found[1]) * (found[2] === "rem" ? 16 : 1) : null;
    const listeners = new Map<EventListener, () => void>();
    return {
      get matches() {
        return min !== null ? viewportWidth >= min : false;
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

/** The chat area is `width` x `height` px (jsdom has no layout); null removes the stub. */
let chatArea: { width: number; height: number } | null = null;
const realRect = Element.prototype.getBoundingClientRect;
function sizeChatArea(width: number, height = 900) {
  chatArea = { width, height };
  act(() => {
    window.dispatchEvent(new Event("resize"));
  });
}

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

let previous: PersistAdapter;
beforeEach(() => {
  clearPanels();
  clearShortcuts();
  localStorage.clear();
  resetTabsStore();
  resetChatDrafts();
  reloadLayoutPrefs();
  chatArea = null;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (chatArea && (this as HTMLElement).dataset?.testid === "tabbed-chat") {
      return { x: 0, y: 0, top: 0, left: 0, right: chatArea.width, bottom: chatArea.height, width: chatArea.width, height: chatArea.height, toJSON: () => ({}) };
    }
    return realRect.call(this);
  });
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  registerSlot("app-shell", AppShell);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  endTabDrag();
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
  resetTabsStore();
  stubViewport(1280);
});

function mount(width: number, path: string) {
  stubViewport(width);
  connectMockDesktop({ seed: seedWorkspace });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, view };
}

/** Gives one element its own rectangle (a spy on the element would replace the shared prototype stub). */
function stubRect(element: Element, rect: { left: number; top: number; width: number; height: number }) {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => ({ ...rect, x: rect.left, y: rect.top, right: rect.left + rect.width, bottom: rect.top + rect.height, toJSON: () => ({}) }),
  });
}

/** A drag event with a pointer position (jsdom has no DragEvent, so the coordinates are set on the event). */
function drag(type: "dragOver" | "drop", element: Element, dataTransfer: object, at: { clientX?: number; clientY?: number } = {}) {
  const event = createEvent[type](element, { dataTransfer });
  Object.defineProperty(event, "clientX", { value: at.clientX ?? 0 });
  Object.defineProperty(event, "clientY", { value: at.clientY ?? 0 });
  fireEvent(element, event);
}

const S = (n: number | string) => `/s/sess-seed-${n}`;
const path = (router: ReturnType<typeof mount>["router"]) => router.state.location.pathname;
const goTo = async (router: ReturnType<typeof mount>["router"], to: string) => {
  await act(async () => {
    await router.navigate({ to: `/d/${DESKTOP_ID}${to}` as never });
  });
};
const tabKeys = (scope: HTMLElement | Document = document) => within(scope as HTMLElement).queryAllByRole("tab").map((tab) => tab.getAttribute("data-key"));
const selectedKey = () => screen.queryAllByRole("tab", { selected: true }).map((tab) => tab.getAttribute("data-key"));
const key = (code: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(window, { code, key: code, ctrlKey: true, ...init });
const stored = () => JSON.parse(localStorage.getItem(tabsKey(DESKTOP_ID)) ?? "null") as { ws: { panes: Record<string, { tabs: string[]; active: string | null }> } } | null;

async function openThree() {
  const view = mount(1280, S(0));
  await screen.findByRole("tablist");
  await goTo(view.router, S(1));
  await goTo(view.router, S(2));
  await waitFor(() => expect(tabKeys()).toHaveLength(3));
  return view;
}

describe("tabs", () => {
  it("opens a tab for the session of the URL and one more for every session visited, and marks the current one", async () => {
    const { router } = await openThree();
    expect(tabKeys()).toEqual([sessionTab("sess-seed-0"), sessionTab("sess-seed-1"), sessionTab("sess-seed-2")]);
    expect(selectedKey()).toEqual([sessionTab("sess-seed-2")]);
    expect(screen.getByRole("tablist", { name: "Open chats" })).toBeInTheDocument();
    await goTo(router, S(0));
    expect(selectedKey()).toEqual([sessionTab("sess-seed-0")]);
    expect(tabKeys()).toHaveLength(3);
  });

  it("switches tabs by click and the URL follows", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    await user.click(screen.getAllByRole("tab")[0]!);
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(0)}`));
    expect(selectedKey()).toEqual([sessionTab("sess-seed-0")]);
    // Only the active tab's screen is mounted.
    expect(screen.getAllByTestId("route-stub")).toHaveLength(1);
  });

  it("walks the tabs with the arrow keys, Home and End, with one tab stop", async () => {
    const user = userEvent.setup();
    await openThree();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.tabIndex)).toEqual([-1, -1, 0]);
    tabs[2]!.focus();
    await user.keyboard("{ArrowLeft}");
    expect(tabs[1]).toHaveFocus();
    await user.keyboard("{Home}");
    expect(tabs[0]).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(tabs[2]).toHaveFocus();
  });

  it("closes a tab with its close button, the neighbour takes over and the URL follows", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    await goTo(router, S(1));
    await user.click(screen.getAllByTestId("tab-close")[1]!);
    await waitFor(() => expect(tabKeys()).toEqual([sessionTab("sess-seed-0"), sessionTab("sess-seed-2")]));
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(2)}`));
    expect(selectedKey()).toEqual([sessionTab("sess-seed-2")]);
  });

  it("closes a tab with a middle click", async () => {
    const { router } = await openThree();
    fireEvent(screen.getAllByRole("tab")[2]!, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(1)}`));
  });

  it("closes the tab with Mod+Alt+W and leaves for the project when the last one closes", async () => {
    const { router } = await openThree();
    key("KeyW", { altKey: true });
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
    key("KeyW", { altKey: true });
    key("KeyW", { altKey: true });
    await waitFor(() => expect(path(router)).toMatch(/\/p\/proj-alpha\/?$/));
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("cycles and jumps with Mod+Alt+brackets and digits", async () => {
    const { router } = await openThree();
    key("BracketRight", { altKey: true });
    await waitFor(() => expect(selectedKey()).toEqual([sessionTab("sess-seed-0")]));
    key("BracketLeft", { altKey: true });
    await waitFor(() => expect(selectedKey()).toEqual([sessionTab("sess-seed-2")]));
    key("Digit2", { altKey: true });
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(1)}`));
    key("Digit9", { altKey: true });
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(2)}`));
  });

  it("reorders with Alt+Arrow and by dragging a tab onto another", async () => {
    const user = userEvent.setup();
    await openThree();
    const first = () => screen.getAllByRole("tab")[0]!;
    first().focus();
    await user.keyboard("{Alt>}{ArrowRight}{/Alt}");
    await waitFor(() => expect(tabKeys()).toEqual([sessionTab("sess-seed-1"), sessionTab("sess-seed-0"), sessionTab("sess-seed-2")]));

    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };
    const last = screen.getAllByRole("tab")[2]!;
    fireEvent.dragStart(last, { dataTransfer });
    // The pointer is left of the middle of the first tab: drop before it.
    const target = screen.getAllByRole("tab")[0]!.parentElement!;
    stubRect(target, { left: 100, width: 100, top: 0, height: 36 });
    drag("dragOver", target, dataTransfer, { clientX: 110 });
    drag("drop", target, dataTransfer, { clientX: 110 });
    fireEvent.dragEnd(last, { dataTransfer });
    await waitFor(() => expect(tabKeys()).toEqual([sessionTab("sess-seed-2"), sessionTab("sess-seed-1"), sessionTab("sess-seed-0")]));
  });

  it("persists the tabs per desktop and restores them on the next visit", async () => {
    const first = await openThree();
    expect(stored()?.ws.panes.p1?.tabs).toHaveLength(3);
    first.view.unmount();
    resetLinkConnections();
    clearLinkStores();
    resetTabsStore();
    mount(1280, "/settings");
    await screen.findByRole("tablist");
    expect(tabKeys()).toHaveLength(3);
    // Not a tab URL: nothing is selected and the settings page shows in the pane.
    expect(selectedKey()).toEqual([]);
    expect(localStorage.getItem(tabsKey("other-desktop"))).toBeNull();
  });

  it("shows a tab for a session the catalog does not list under a plain name", async () => {
    mount(1280, "/s/gone-session");
    const tab = await screen.findByRole("tab");
    expect(tab).toHaveTextContent("Session");
  });

  it("handles overflow: the strip scrolls sideways and keeps the selected tab in view", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const { router } = mount(1280, S(0));
    await screen.findByRole("tablist");
    for (let i = 1; i <= 8; i++) await goTo(router, S(i));
    await waitFor(() => expect(tabKeys()).toHaveLength(9));
    const list = screen.getByRole("tablist");
    expect(list.className).toContain("overflow-x-auto");
    expect(screen.getAllByRole("tab")[0]!.parentElement!.className).toContain("shrink-0");
    expect(scroll).toHaveBeenCalled();
    fireEvent.wheel(list, { deltaY: 40 });
    expect(list.scrollLeft).toBe(40);
  });

  it("gives every control a name and a 44px target on touch", async () => {
    await openThree();
    // The close buttons are hidden from assistive tech (the tablist holds only tabs) but keep a title and a 44px touch target.
    for (const button of screen.getAllByTestId("tab-close")) {
      expect(button.className).toContain("pointer-coarse:w-11");
      expect(button).toHaveAttribute("title", expect.stringMatching(/^Close /));
    }
    expect(screen.getAllByRole("tab")[0]!.parentElement!.className).toContain("h-11");
    expect(screen.getByRole("button", { name: "Split right" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Split down" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getAllByRole("main")).toHaveLength(1);
  });
});

describe("tab strip structure and keyboard", () => {
  it("keeps only tabs inside the tablist (aria-required-children): the close buttons are hidden from assistive tech", async () => {
    await openThree();
    const list = screen.getByRole("tablist");
    // Nothing a screen reader can reach inside the tablist but tabs.
    expect(within(list).queryAllByRole("button")).toEqual([]);
    expect(within(list).getAllByRole("tab")).toHaveLength(3);
    for (const close of screen.getAllByTestId("tab-close")) {
      expect(close).toHaveAttribute("aria-hidden", "true");
      expect(close).toHaveAttribute("tabindex", "-1");
    }
    // The session header's action group yields to the title: it can shrink and is capped.
    expect(screen.getByRole("button", { name: "Session actions" }).parentElement!.className).toContain("max-w-[55%]");
    for (const tab of screen.getAllByRole("tab")) expect(tab).toHaveAttribute("aria-keyshortcuts", "Delete Backspace");
  });

  it("closes the focused tab with Delete and moves the focus to its neighbour", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    await goTo(router, S(1));
    const tabs = screen.getAllByRole("tab");
    tabs[1]!.focus();
    await user.keyboard("{Delete}");
    await waitFor(() => expect(tabKeys()).toEqual([sessionTab("sess-seed-0"), sessionTab("sess-seed-2")]));
    await waitFor(() => expect(screen.getAllByRole("tab")[1]).toHaveFocus());
    expect(screen.getAllByRole("tab")[1]).toHaveAttribute("data-key", sessionTab("sess-seed-2"));
    // The last tab closing leaves the focus on the new last one.
    await user.keyboard("{Delete}");
    await waitFor(() => expect(tabKeys()).toEqual([sessionTab("sess-seed-0")]));
    await waitFor(() => expect(screen.getAllByRole("tab")[0]).toHaveFocus());
  });

  it("shows edge fades and an All tabs menu when the tabs overflow, and the menu opens any tab", async () => {
    const user = userEvent.setup();
    const width = vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "tablist" ? 1400 : 0;
    });
    const client = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(function (this: HTMLElement) {
      return this.getAttribute("role") === "tablist" ? 300 : 0;
    });
    try {
      const { router } = await openThree();
      expect(await screen.findByTestId("tabs-fade-right")).toBeInTheDocument();
      expect(screen.queryByTestId("tabs-fade-left")).toBeNull();
      // The fades and the menu button are outside the tablist.
      expect(within(screen.getByRole("tablist")).queryByTestId("tabs-fade-right")).toBeNull();
      const list = screen.getByRole("tablist");
      list.scrollLeft = 200;
      fireEvent.scroll(list);
      expect(await screen.findByTestId("tabs-fade-left")).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "All tabs" }));
      const menu = await screen.findByRole("menu");
      // An item per tab to open it, then one per tab to close it.
      expect(within(menu).getAllByRole("menuitem")).toHaveLength(6);
      await user.click(within(menu).getAllByRole("menuitem")[0]!);
      await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(0)}`));
    } finally {
      width.mockRestore();
      client.mockRestore();
    }
  });

  it("shows no fades while the tabs fit, but the All tabs menu can still close a tab (no close button for a screen reader)", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    expect(screen.queryByTestId("tabs-fade-right")).toBeNull();
    await user.click(screen.getByRole("button", { name: "All tabs" }));
    const menu = await screen.findByRole("menu");
    await user.click(within(menu).getAllByRole("menuitem", { name: /^Close / })[0]!);
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
    await waitFor(() => expect(path(router)).toMatch(/\/s\//));
  });

  it("closes the focused tab with Backspace too (the Mac Delete key)", async () => {
    const user = userEvent.setup();
    await openThree();
    screen.getAllByRole("tab")[2]!.focus();
    await user.keyboard("{Backspace}");
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
  });

  it("does not close tabs on a held key or with a modifier (holding Backspace must not close them all)", async () => {
    await openThree();
    const tab = screen.getAllByRole("tab")[2]!;
    tab.focus();
    fireEvent.keyDown(tab, { key: "Backspace", repeat: true });
    fireEvent.keyDown(tab, { key: "Delete", repeat: true });
    fireEvent.keyDown(tab, { key: "Backspace", ctrlKey: true });
    fireEvent.keyDown(tab, { key: "Delete", metaKey: true });
    fireEvent.keyDown(tab, { key: "Delete", altKey: true });
    expect(tabKeys()).toHaveLength(3);
    fireEvent.keyDown(tab, { key: "Delete" });
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
  });

  it("scrolls the selected tab back into view when the strip is resized or moved (the observer fires)", async () => {
    // Only the observers of the tab strip (the virtualized transcript has its own).
    const callbacks: Array<() => void> = [];
    vi.stubGlobal(
      "ResizeObserver",
      class {
        callback: () => void;
        constructor(callback: () => void) {
          this.callback = callback;
        }
        observe(element: Element) {
          if (element.getAttribute("role") === "tablist") callbacks.push(this.callback);
        }
        unobserve() {}
        disconnect() {}
      },
    );
    try {
      await openThree();
      const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
      scroll.mockClear();
      act(() => {
        for (const callback of callbacks) callback();
      });
      expect(scroll).toHaveBeenCalled();
      expect((scroll.mock.contexts as Element[]).some((node) => node.getAttribute("data-key") === sessionTab("sess-seed-2"))).toBe(true);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("hands the focus to the new pane when a split starts inside a composer, and keeps the scroll offset of the old one", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    const box = (await screen.findByRole("textbox", { name: "Message" })) as HTMLTextAreaElement;
    screen.getByTestId("pane-body").scrollTop = 120;
    await user.click(box);
    // A real browser resets scroll offsets of a node that is re-inserted: model that.
    const append = Node.prototype.appendChild;
    vi.spyOn(Node.prototype, "appendChild").mockImplementation(function (this: Node, node: Node) {
      const result = append.call(this, node);
      if (node instanceof HTMLElement && node.style.display === "contents") for (const element of node.querySelectorAll("*")) element.scrollTop = 0;
      return result;
    } as typeof append);
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    expect(within(screen.getByTestId("pane-p1")).getByTestId("pane-body").scrollTop).toBe(120);
    // The focus stays with the new pane: not given back to the old composer, and the URL moves to New session.
    await waitFor(() => expect(path(router)).toMatch(/\/p\/proj-alpha\/new$/));
    expect(getWorkspace(DESKTOP_ID).focused).toBe("p2");
    expect(box).not.toHaveFocus();
    expect(await within(screen.getByTestId("pane-p2")).findByTestId("new-session-screen")).toBeInTheDocument();
  });

  it("keeps the focus of a pane that stays the focused one when the layout moves it", async () => {
    const user = userEvent.setup();
    await openThree();
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    const box = within(screen.getByTestId("pane-p1")).getByRole("textbox", { name: "Message" });
    await user.click(box);
    expect(getWorkspace(DESKTOP_ID).focused).toBe("p1");
    // Too narrow for two panes: only p1 shows, and its host moves.
    sizeChatArea(500);
    await waitFor(() => expect(screen.getByTestId("tabbed-chat")).toHaveAttribute("data-panes", "1"));
    expect(within(screen.getByTestId("pane-p1")).getByRole("textbox", { name: "Message" })).toBe(box);
    expect(box).toHaveFocus();
  });

  it("shows this page's URL tab in its pane when another browser tab moved that tab to another pane", async () => {
    await openThree();
    // The other browser tab put sess-seed-2 in a second pane and left its focus and active tab elsewhere.
    let other = openTab(openTab(openTab(emptyWorkspace(), sessionTab("sess-seed-0")), sessionTab("sess-seed-1")), sessionTab("sess-seed-2"));
    other = splitPane(other, "p1", "row");
    other = moveTab(other, sessionTab("sess-seed-2"), "p2");
    other = activateTab(other, "p1", sessionTab("sess-seed-0"));
    expect(other.focused).toBe("p1");
    localStorage.setItem(tabsKey(DESKTOP_ID), serializeWorkspace(other));
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: tabsKey(DESKTOP_ID) }));
    });
    await waitFor(() => expect(getWorkspace(DESKTOP_ID).focused).toBe("p2"));
    expect(getWorkspace(DESKTOP_ID).panes.p2?.active).toBe(sessionTab("sess-seed-2"));
    expect(screen.getByTestId("pane-p2")).toHaveAttribute("data-focused", "true");
    // Once the URL tab shows, a second event writes nothing.
    const written = localStorage.getItem(tabsKey(DESKTOP_ID));
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: tabsKey(DESKTOP_ID) }));
    });
    expect(localStorage.getItem(tabsKey(DESKTOP_ID))).toBe(written);
  });

  it("keeps this page's focused pane and active tab when another browser tab changes them (no write back)", async () => {
    await openThree();
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    const here = getWorkspace(DESKTOP_ID);
    // Another browser tab activated sess-seed-0 in p1 and focused it, and opened a tab of its own.
    const other = JSON.parse(localStorage.getItem(tabsKey(DESKTOP_ID))!);
    other.ws.panes.p1.active = sessionTab("sess-seed-0");
    other.ws.focused = "p1";
    other.ws.panes.p2.tabs = [sessionTab("sess-seed-9")];
    other.ws.panes.p2.active = sessionTab("sess-seed-9");
    const raw = JSON.stringify(other);
    localStorage.setItem(tabsKey(DESKTOP_ID), raw);
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: tabsKey(DESKTOP_ID) }));
    });
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toEqual([sessionTab("sess-seed-9")]));
    // Their tab sets are adopted; the focus and the active tab stay what this page had.
    expect(getWorkspace(DESKTOP_ID).focused).toBe(here.focused);
    expect(getWorkspace(DESKTOP_ID).panes.p1?.active).toBe(here.panes.p1?.active);
    expect(localStorage.getItem(tabsKey(DESKTOP_ID))).toBe(raw);
  });

  it("does not leave a page that is not a tab when a tab closes or moves", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    await goTo(router, "/p/proj-alpha/new");
    await screen.findByTestId("new-session-screen");
    await user.click(screen.getAllByTestId("tab-close")[0]!);
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
    expect(path(router)).toBe(`/d/${DESKTOP_ID}/p/proj-alpha/new`);
    // Reordering with Alt+Arrow does not navigate either.
    screen.getAllByRole("tab")[0]!.focus();
    await user.keyboard("{Alt>}{ArrowRight}{/Alt}");
    await waitFor(() => expect(tabKeys()[1]).toBe(sessionTab("sess-seed-1")));
    expect(path(router)).toBe(`/d/${DESKTOP_ID}/p/proj-alpha/new`);
    expect(screen.getByTestId("new-session-screen")).toBeInTheDocument();
  });

  it("adopts the workspace another browser tab wrote (storage event)", async () => {
    await openThree();
    const other = openTab(openTab(emptyWorkspace(), sessionTab("sess-seed-7")), sessionTab("sess-seed-8"));
    localStorage.setItem(tabsKey(DESKTOP_ID), serializeWorkspace(other));
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: tabsKey(DESKTOP_ID) }));
    });
    // The other tab's tabs are shown at once, and this page's own URL tab (not in them) is added once.
    await waitFor(() => expect(tabKeys()).toEqual([sessionTab("sess-seed-7"), sessionTab("sess-seed-8"), sessionTab("sess-seed-2")]));
  });

  it("keeps a pane mounted when a split or a closed pane changes the layout around it", async () => {
    const user = userEvent.setup();
    await openThree();
    const box = (await screen.findByRole("textbox", { name: "Message" })) as HTMLTextAreaElement;
    await user.type(box, "keep me");
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    // The same element, with the text still in it: the pane was moved, not rebuilt.
    expect(within(screen.getByTestId("pane-p1")).getByRole("textbox", { name: "Message" })).toBe(box);
    expect(box.value).toBe("keep me");
    await user.click(within(screen.getByTestId("pane-p2")).getByRole("button", { name: "Close pane" }));
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(1));
    expect(screen.getByRole("textbox", { name: "Message" })).toBe(box);
    expect(box.value).toBe("keep me");
  });

  it("keeps what was typed in a session when its tab is left and opened again", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    await user.type(await screen.findByRole("textbox", { name: "Message" }), "half a thought");
    await goTo(router, S(0));
    await waitFor(() => expect(selectedKey()).toEqual([sessionTab("sess-seed-0")]));
    expect(await screen.findByRole("textbox", { name: "Message" })).toHaveValue("");
    await goTo(router, S(2));
    await waitFor(() => expect(selectedKey()).toEqual([sessionTab("sess-seed-2")]));
    expect(await screen.findByRole("textbox", { name: "Message" })).toHaveValue("half a thought");
  });

  it("drops the draft of a sent message, so a remount cannot bring it back to be sent twice", async () => {
    const user = userEvent.setup();
    await openThree();
    const box = await screen.findByRole("textbox", { name: "Message" });
    await user.type(box, "send me once");
    expect(readChatDraft(DESKTOP_ID, "sess-seed-2")).toBe("send me once");
    await user.keyboard("{Control>}{Enter}{/Control}");
    await waitFor(() => expect(readChatDraft(DESKTOP_ID, "sess-seed-2")).toBe(""));
  });

  it("mounts the drop zones over the pane, outside the scrolling body, and a drop on the tab's own pane moves nothing", async () => {
    await openThree();
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };
    fireEvent.dragStart(screen.getAllByRole("tab")[0]!, { dataTransfer });
    const zones = await screen.findByTestId("drop-zones-p1");
    const body = screen.getByTestId("pane-body");
    expect(zones.parentElement).toContainElement(body);
    expect(zones.parentElement).not.toBe(body);
    expect(zones.parentElement!.className).not.toContain("overflow-y-auto");
    drag("dragOver", zones, dataTransfer);
    drag("drop", zones, dataTransfer);
    expect(tabKeys()).toEqual([sessionTab("sess-seed-0"), sessionTab("sess-seed-1"), sessionTab("sess-seed-2")]);
  });
});

describe("tab shortcuts and AltGr", () => {
  it("still closes a tab with Ctrl+Alt+W outside inputs, whatever the browser reports for AltGraph", async () => {
    await openThree();
    fireEvent.keyDown(window, { code: "KeyW", key: "w", ctrlKey: true, altKey: true });
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
    // Firefox reports AltGraph for every Ctrl+Alt on Windows: still a shortcut when the key typed nothing new.
    fireEvent.keyDown(window, { code: "KeyW", key: "w", ctrlKey: true, altKey: true, modifierAltGraph: true });
    await waitFor(() => expect(tabKeys()).toHaveLength(1));
  });

  it("does not swallow AltGr characters: Ctrl+Alt+Digit2 typing \"@\" and Ctrl+Alt+Digit8 typing \"[\"", async () => {
    await openThree();
    const event = fireEvent.keyDown(window, { code: "Digit2", key: "@", ctrlKey: true, altKey: true, modifierAltGraph: true });
    expect(event).toBe(true);
    expect(fireEvent.keyDown(window, { code: "Digit8", key: "[", ctrlKey: true, altKey: true })).toBe(true);
    expect(fireEvent.keyDown(window, { code: "BracketRight", key: "~", ctrlKey: true, altKey: true })).toBe(true);
    expect(tabKeys()).toHaveLength(3);
    expect(selectedKey()).toEqual([sessionTab("sess-seed-2")]);
    // The same chord with the plain character is the shortcut (and is cancelled).
    expect(fireEvent.keyDown(window, { code: "Digit2", key: "2", ctrlKey: true, altKey: true })).toBe(false);
  });

  it("leaves the Mod+Alt chords to a focused text field off the Mac, but still takes Mod+D", async () => {
    await openThree();
    const input = document.createElement("input");
    document.body.appendChild(input);
    try {
      fireEvent.keyDown(input, { code: "KeyW", key: "w", ctrlKey: true, altKey: true });
      expect(tabKeys()).toHaveLength(3);
      fireEvent.keyDown(input, { code: "KeyD", key: "d", ctrlKey: true });
      await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    } finally {
      input.remove();
    }
  });
});

describe("split panes", () => {
  it("splits right with Mod+D: a second pane with the New session page, and the tabs of the first stay", async () => {
    const { router } = await openThree();
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    expect(paneOrder(getWorkspace(DESKTOP_ID).root)).toEqual(["p1", "p2"]);
    expect(screen.getByTestId("pane-split")).toHaveAttribute("data-dir", "row");
    expect(screen.getByRole("separator", { name: "Resize panes" })).toBeInTheDocument();
    await waitFor(() => expect(path(router)).toMatch(/\/p\/proj-alpha\/new$/));
    expect(screen.getByTestId("pane-p2")).toHaveAttribute("data-focused", "true");
    expect(await within(screen.getByTestId("pane-p2")).findByTestId("new-session-screen")).toBeInTheDocument();
    expect(tabKeys(screen.getByTestId("pane-p1"))).toHaveLength(3);
    expect(screen.getByTestId("tabbed-chat")).toHaveAttribute("data-panes", "2");
  });

  it("splits down with Mod+Shift+D and with the button", async () => {
    const user = userEvent.setup();
    await openThree();
    key("KeyD", { shiftKey: true });
    await waitFor(() => expect(screen.getByTestId("pane-split")).toHaveAttribute("data-dir", "column"));
    await user.click(within(screen.getByTestId("pane-p2")).getByRole("button", { name: "Split right" }));
    await waitFor(() => expect(paneOrder(getWorkspace(DESKTOP_ID).root)).toHaveLength(3));
    expect(screen.getAllByTestId("pane-split")).toHaveLength(2);
  });

  it("a session opened from the list lands in the focused pane, and the focused pane drives the URL", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    await goTo(router, S(5));
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toEqual([sessionTab("sess-seed-5")]));
    expect(tabKeys(screen.getByTestId("pane-p1"))).toHaveLength(3);
    // Clicking a tab of the other pane focuses it and navigates to it.
    await user.click(within(screen.getByTestId("pane-p1")).getAllByRole("tab")[0]!);
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(0)}`));
    expect(screen.getByTestId("pane-p1")).toHaveAttribute("data-focused", "true");
    expect(getWorkspace(DESKTOP_ID).focused).toBe("p1");
    // Navigating to a session that is open in the other pane focuses that pane instead of opening it twice.
    await goTo(router, S(5));
    await waitFor(() => expect(getWorkspace(DESKTOP_ID).focused).toBe("p2"));
    expect(tabKeys()).toHaveLength(4);
  });

  it("has one h1: the focused pane's, the other pane's title is an h2", async () => {
    const { router } = await openThree();
    key("KeyD");
    await goTo(router, S(5));
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toHaveLength(1));
    await waitFor(() => expect(within(screen.getByTestId("pane-p2")).getByRole("heading", { level: 1 })).toBeInTheDocument());
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(within(screen.getByTestId("pane-p1")).getAllByRole("heading", { level: 2 }).length).toBeGreaterThan(0);
    expect(screen.getAllByRole("main")).toHaveLength(1);
  });

  it("moves a tab to another pane by dropping it on the pane, and an edge drop splits the pane", async () => {
    const { router } = await openThree();
    key("KeyD");
    await goTo(router, S(5));
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toHaveLength(1));
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };
    const dragged = within(screen.getByTestId("pane-p1")).getAllByRole("tab")[0]!;
    fireEvent.dragStart(dragged, { dataTransfer });
    const zones = await screen.findByTestId("drop-zones-p2");
    drag("dragOver", zones, dataTransfer);
    drag("drop", zones, dataTransfer);
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toEqual([sessionTab("sess-seed-5"), sessionTab("sess-seed-0")]));
    expect(tabKeys(screen.getByTestId("pane-p1"))).toHaveLength(2);
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(0)}`));

    // Dropping on the lower edge of a pane splits it downwards (the tab leaves its pane and fills the new half).
    const second = within(screen.getByTestId("pane-p1")).getAllByRole("tab")[0]!;
    fireEvent.dragStart(second, { dataTransfer });
    const target = await screen.findByTestId("drop-zones-p2");
    stubRect(target, { left: 0, top: 0, width: 400, height: 400 });
    sizeChatArea(1400, 1200);
    drag("dragOver", target, dataTransfer, { clientX: 200, clientY: 390 });
    drag("drop", target, dataTransfer, { clientX: 200, clientY: 390 });
    await waitFor(() => expect(screen.getAllByTestId("pane-split").some((node) => node.getAttribute("data-dir") === "column")).toBe(true));
  });

  it("closes a pane with its button, with its last tab, and closes nothing when it is the only pane", async () => {
    const user = userEvent.setup();
    const { router } = await openThree();
    expect(screen.queryByRole("button", { name: "Close pane" })).toBeNull();
    key("KeyD");
    await goTo(router, S(5));
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toHaveLength(1));
    await user.click(within(screen.getByTestId("pane-p2")).getAllByTestId("tab-close")[0]!);
    await waitFor(() => expect(paneOrder(getWorkspace(DESKTOP_ID).root)).toEqual(["p1"]));
    expect(screen.queryByRole("button", { name: "Close pane" })).toBeNull();

    key("KeyD");
    await waitFor(() => expect(paneOrder(getWorkspace(DESKTOP_ID).root)).toHaveLength(2));
    await user.click(within(screen.getByTestId("pane-p3")).getByRole("button", { name: "Close pane" }));
    await waitFor(() => expect(paneOrder(getWorkspace(DESKTOP_ID).root)).toEqual(["p1"]));
    expect(getWorkspace(DESKTOP_ID).focused).toBe("p1");
  });

  it("moves the focus between panes with Mod+Alt+Shift+Arrow", async () => {
    const { router } = await openThree();
    key("KeyD");
    await goTo(router, S(5));
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toHaveLength(1));
    key("ArrowLeft", { altKey: true, shiftKey: true });
    await waitFor(() => expect(getWorkspace(DESKTOP_ID).focused).toBe("p1"));
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(2)}`));
    key("ArrowRight", { altKey: true, shiftKey: true });
    await waitFor(() => expect(path(router)).toBe(`/d/${DESKTOP_ID}${S(5)}`));
  });

  it("resizes the panes with the divider and remembers the sizes", async () => {
    const user = userEvent.setup();
    await openThree();
    key("KeyD");
    const divider = await screen.findByRole("separator", { name: "Resize panes" });
    expect(divider).toHaveAttribute("aria-valuenow", "50");
    divider.focus();
    await user.keyboard("{ArrowRight}");
    expect(divider).toHaveAttribute("aria-valuenow", "52");
    expect(JSON.parse(localStorage.getItem(tabsKey(DESKTOP_ID))!).ws.root.sizes[0]).toBeCloseTo(0.52);
  });

  it("restores the panes of the desktop on the next visit", async () => {
    const first = await openThree();
    key("KeyD");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    first.view.unmount();
    resetLinkConnections();
    clearLinkStores();
    resetTabsStore();
    mount(1280, "/settings");
    await waitFor(() => expect(screen.getAllByRole("tablist")).toHaveLength(2));
    expect(tabKeys(screen.getByTestId("pane-p1"))).toHaveLength(3);
  });

  it("shows only the focused pane while the window is too narrow, and brings the others back when it grows", async () => {
    const { router } = await openThree();
    key("KeyD");
    await goTo(router, S(5));
    await waitFor(() => expect(tabKeys(screen.getByTestId("pane-p2"))).toHaveLength(1));
    sizeChatArea(500);
    await waitFor(() => expect(screen.getByTestId("tabbed-chat")).toHaveAttribute("data-panes", "1"));
    expect(screen.queryByTestId("pane-p1")).toBeNull();
    expect(screen.getByTestId("hidden-panes")).toHaveTextContent("+1 hidden");
    expect(screen.queryByRole("separator", { name: "Resize panes" })).toBeNull();
    sizeChatArea(900);
    await waitFor(() => expect(screen.getByTestId("tabbed-chat")).toHaveAttribute("data-panes", "2"));
    expect(screen.queryByTestId("hidden-panes")).toBeNull();
  });

  it("does not split where two panes cannot fit", async () => {
    await openThree();
    sizeChatArea(500);
    expect(await screen.findByRole("button", { name: "Split right" })).toBeDisabled();
    key("KeyD");
    expect(paneOrder(getWorkspace(DESKTOP_ID).root)).toEqual(["p1"]);
    sizeChatArea(900, 500);
    await waitFor(() => expect(screen.getByRole("button", { name: "Split right" })).toBeEnabled());
    expect(screen.getByRole("button", { name: "Split down" })).toBeDisabled();
  });
});

describe("widths", () => {
  it("has no tabs and no split below 640, and keeps single-session navigation", async () => {
    mount(390, S(0));
    await screen.findByTestId("route-stub");
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.queryByTestId("tabbed-chat")).toBeNull();
    expect(screen.queryByRole("button", { name: "Split right" })).toBeNull();
    fireEvent.keyDown(window, { code: "KeyD", key: "d", ctrlKey: true });
    expect(localStorage.getItem(tabsKey(DESKTOP_ID))).toBeNull();
  });

  it("has tabs on a tablet, and splits only when the chat area is wide enough", async () => {
    const { router } = mount(1000, S(0));
    await screen.findByRole("tablist");
    await goTo(router, S(1));
    await waitFor(() => expect(tabKeys()).toHaveLength(2));
    sizeChatArea(700);
    await waitFor(() => expect(screen.getByRole("button", { name: "Split right" })).toBeEnabled());
    sizeChatArea(450);
    await waitFor(() => expect(screen.getByRole("button", { name: "Split right" })).toBeDisabled());
  });

  it("drops the tabs when the window becomes a phone and brings them back", async () => {
    const view = mount(1280, S(0));
    await screen.findByRole("tablist");
    act(() => {
      stubViewport(390);
      window.dispatchEvent(new Event("resize"));
      for (const listener of mediaListeners) listener();
    });
    await waitFor(() => expect(screen.queryByRole("tablist")).toBeNull());
    expect(view.router.state.location.pathname).toBe(`/d/${DESKTOP_ID}${S(0)}`);
    act(() => {
      stubViewport(1280);
      window.dispatchEvent(new Event("resize"));
      for (const listener of mediaListeners) listener();
    });
    expect(await screen.findByRole("tablist")).toBeInTheDocument();
  });
});
