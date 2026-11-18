// U-02: the app shell at 390 (passthrough), 768 (tablet) and 1280 (desktop), the right-panel host, the
// shortcuts and the command palette, against the real router and the mock desktop.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { clearHooks, registerSessionCapabilities } from "@/ui/hooks";
import { clearSlots, registerSlot } from "@/ui/slots";
import { AppShell } from "./AppShell";
import { PANEL_WIDTH_KEY, SIDEBAR_COLLAPSED_KEY, SIDEBAR_WIDTH_KEY, dockedPanelMax, reloadLayoutPrefs, useLayoutPrefs } from "./layoutPrefs";
import { closeCommandPalette, isCommandPaletteOpen, openCommandPalette } from "./palette";
import { clearPanels, closePanel, openPanel, registerPanel, usePanels } from "./panels";
import { clearShortcuts, isMod, registerShortcut } from "./shortcuts";

const mediaListeners = new Set<() => void>();
let viewportWidth = 1280;

/** matchMedia for a viewport `width` px wide: every `(min-width: Npx)` query matches when width >= N. */
function stubViewport(width: number) {
  viewportWidth = width;
  Object.defineProperty(window, "innerWidth", { configurable: true, writable: true, value: width });
  window.matchMedia = ((query: string) => {
    const min = /min-width:\s*(\d+)px/.exec(query);
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

let previous: PersistAdapter;
beforeEach(() => {
  localStorage.clear();
  reloadLayoutPrefs();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  registerSlot("app-shell", AppShell);
});
afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  clearPanels();
  clearShortcuts();
  closeCommandPalette();
  localStorage.clear();
  stubViewport(1280);
});

function mount(width: number, path = "/settings") {
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

  it.each(["", "/p/proj-alpha", "/cloud"])("adds no second sidebar on %j, which renders WorkspaceFrame", async (path) => {
    mount(1280, path);
    await screen.findByTestId("app-shell");
    await waitFor(() => expect(screen.getAllByTestId("workspace-nav")).toHaveLength(1));
    expect(screen.queryByTestId("app-sidebar")).toBeNull();
  });

  it("opens the palette from the sidebar button", async () => {
    const user = userEvent.setup();
    mount(1280);
    const sidebar = await screen.findByTestId("app-sidebar");
    await user.click(within(sidebar).getByRole("button", { name: "Search" }));
    expect(await screen.findByRole("combobox")).toBeInTheDocument();
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

  it("is docked and resizable at 1280, clamped to 380-480", async () => {
    const user = userEvent.setup();
    mount(1280, ALPHA);
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

  it("keeps 320px for the main pane: narrows the docked panel at 1024, falls back to the overlay when 380 does not fit", async () => {
    expect(dockedPanelMax(1024, 320)).toBe(384);
    expect(dockedPanelMax(1024, 360)).toBeNull();
    expect(dockedPanelMax(1100, 320)).toBe(460);
    expect(dockedPanelMax(1280, 320)).toBe(480);
    expect(dockedPanelMax(1280, 360)).toBe(480);

    mount(1024, ALPHA); // WorkspaceFrame draws its own 320px sidebar here
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target, local));
    const docked = await screen.findByTestId("panel-docked");
    expect(docked).toHaveStyle({ width: "384px" });
    expect(screen.getByRole("separator", { name: "Resize panel" })).toHaveAttribute("aria-valuemax", "384");
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
      localStorage.setItem(SIDEBAR_COLLAPSED_KEY, bad);
      reloadLayoutPrefs();
      const state = useLayoutPrefs.getState();
      expect(state.sidebarWidth, bad).toBe(320);
      expect(state.panelWidth, bad).toBe(420);
      expect(state.sidebarCollapsed, bad).toBe(false);
    }
  });

  it("clamps out-of-range numbers", () => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, "10");
    localStorage.setItem(PANEL_WIDTH_KEY, "9000");
    reloadLayoutPrefs();
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

  it("Mod+K and Mod+P do nothing while another dialog is open", async () => {
    mount(1280);
    await screen.findByTestId("app-sidebar");
    const handler = vi.fn();
    registerShortcut("quick-open", handler);
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    document.body.appendChild(dialog);
    expect(press({ key: "k", ctrlKey: true }).defaultPrevented).toBe(false);
    expect(press({ key: "p", ctrlKey: true }).defaultPrevented).toBe(false);
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
