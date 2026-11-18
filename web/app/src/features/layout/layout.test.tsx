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
import { PANEL_WIDTH_KEY, SIDEBAR_COLLAPSED_KEY, SIDEBAR_WIDTH_KEY, reloadLayoutPrefs } from "./layoutPrefs";
import { closeCommandPalette } from "./palette";
import { clearPanels, closePanel, openPanel, registerPanel, usePanels } from "./panels";
import { clearShortcuts, registerShortcut } from "./shortcuts";

/** matchMedia for a viewport `width` px wide: every `(min-width: Npx)` query matches when width >= N. */
function stubViewport(width: number) {
  window.matchMedia = ((query: string) => {
    const min = /min-width:\s*(\d+)px/.exec(query);
    return {
      matches: min ? width >= Number(min[1]) : false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  }) as unknown as typeof window.matchMedia;
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
    mount(390);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target));
    const sheet = await screen.findByRole("dialog", { name: "Files" });
    expect(within(sheet).getByText("Files of proj-alpha")).toBeInTheDocument();
    expect(screen.queryByTestId("panel-overlay")).toBeNull();
  });

  it("is a 360px overlay at 768 that Esc dismisses", async () => {
    const user = userEvent.setup();
    mount(768);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target));
    const overlay = await screen.findByTestId("panel-overlay");
    expect(overlay).toHaveStyle({ width: "360px" });
    expect(screen.queryByRole("separator", { name: "Resize panel" })).toBeNull();
    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("panel-overlay")).toBeNull();
    expect(usePanels.getState().open).toBeNull();
  });

  it("is docked and resizable at 1280, clamped to 380-480", async () => {
    const user = userEvent.setup();
    mount(1280);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target));
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
    mount(1280);
    await screen.findByTestId("app-shell");
    act(() => void openPanel("files", target));
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
    expect(openPanel("changes", target)).toBe(false);
    expect(usePanels.getState().open).toBeNull();
  });

  it("consults session-capabilities for the kind", () => {
    registerPanel("files", { title: "Files", component: FilesPanel });
    registerSessionCapabilities((capability) => capability !== "files");
    expect(openPanel("files", target, { target: { kind: "cloud" }, desktopCapabilities: [] })).toBe(false);
    expect(openPanel("files", target)).toBe(true);
    closePanel();
  });
});
