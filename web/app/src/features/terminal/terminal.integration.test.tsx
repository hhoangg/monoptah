// The terminal against the real link client wired to the in-process mock desktop (a fake shell with the desktop's
// ticket check, grace and exit rules). The passkey ticket comes from a stub of the Worker call (the ceremony has its own tests). Only xterm is replaced by a recording stand-in: jsdom has no canvas.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import type { MockDesktop } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { clearPanels } from "@/features/layout/panels";
import { DESKTOP_ID, DEVICE_ID, connectMockDesktop } from "@/features/sessions/dev/mockLink";
import { ReauthCancelledError } from "@/features/accounts/queries";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { AppShell } from "@/features/layout/AppShell";
import { clearSlots, registerSlot } from "@/ui/slots";
import { resetTabsStore } from "@/features/layout/tabs/tabsStore";
import { resetTerminalSessions } from "./sessions";
import type * as ApiModule from "@/features/desktops/api";
import type * as ThisDeviceModule from "@/features/desktops/thisDevice";
import { RESUME_KEY_PREFIX } from "./resume";
import { FakeXterm } from "./testing";
import type { ScreenHandlers } from "./xtermScreen";

const h = vi.hoisted(() => ({ screens: [] as unknown[], made: null as null | ((handlers: ScreenHandlers) => unknown) }));
const t = vi.hoisted(() => ({ ticket: vi.fn() }));
vi.mock("@/features/desktops/api", async (importOriginal) => ({
  ...(await importOriginal<typeof ApiModule>()),
  requestTerminalTicket: (desktopId: string, deviceId: string, projectId: string) => t.ticket(desktopId, deviceId, projectId),
}));
vi.mock("@/features/desktops/thisDevice", async (importOriginal) => ({
  ...(await importOriginal<typeof ThisDeviceModule>()),
  useThisDevice: () => ({ deviceId: "22222222-2222-4222-8222-222222222222", fingerprint: "fp" }),
}));
vi.mock("./xtermScreen", () => ({
  createXtermScreen: async (_host: HTMLElement, handlers: ScreenHandlers) => {
    const screen = h.made ? h.made(handlers) : null;
    h.screens.push(screen);
    return screen;
  },
}));

let xterm: FakeXterm;
let desktop: MockDesktop;
let deviceFingerprint = "";
let previous: PersistAdapter;

function viewport(width: number, touch = false) {
  window.matchMedia = ((query: string) => {
    const min = /min-width:\s*(\d+)px/.exec(query);
    const max = /max-width:\s*(\d+)px/.exec(query);
    const matches = query.includes("pointer: coarse") ? touch || (max !== null && width <= Number(max[1])) : min ? width >= Number(min[1]) : max ? width <= Number(max[1]) : false;
    return { matches, media: query, onchange: null, addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false };
  }) as unknown as typeof window.matchMedia;
}

vi.setConfig({ testTimeout: 30_000 });

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  h.screens.length = 0;
  resetTabsStore();
  resetTerminalSessions();
  t.ticket.mockReset();
  t.ticket.mockResolvedValue("ticket-xyz");
  h.made = (handlers) => {
    xterm = new FakeXterm();
    xterm.handlers = handlers;
    return xterm;
  };
});
afterEach(() => {
  cleanup();
  resetTabsStore();
  resetTerminalSessions();
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  clearPanels();
  localStorage.clear();
  sessionStorage.clear();
});

function mount({ permission = "control", terminal = true, width = 1280, touch = false, deviceTerminal = true, verify = true, path = `/d/${DESKTOP_ID}` }: { permission?: "read" | "control"; terminal?: boolean; width?: number; touch?: boolean; deviceTerminal?: boolean; verify?: boolean; path?: string } = {}) {
  registerSlot("app-shell", AppShell);
  viewport(width, touch);
  const link = connectMockDesktop({
    permission,
    capabilities: terminal ? [...KNOWN_CAPABILITIES] : KNOWN_CAPABILITIES.filter((name) => name !== "terminal"),
    seed: (mock) => {
      desktop = mock;
    },
  });
  desktop = link.desktop;
  // The desktop spends the ticket at the Worker; here it accepts (or refuses) it itself.
  desktop.terminals.verifyTicket = async () => verify;
  deviceFingerprint = [...desktop.devices.keys()][0] ?? "";
  if (!deviceTerminal) desktop.setDevice(deviceFingerprint, { terminal: false });
  return renderApp(path);
}

/** Renders the app against the link that is already registered (a "reload" renders it again). */
function renderApp(path = `/d/${DESKTOP_ID}`) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [path] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

const chip = () => screen.findByTestId("terminal-chip", {}, { timeout: 10_000 });

/** Clicks the footer chip and the chooser's Open button (the stubbed passkey ticket is granted at once). */
async function startOpen() {
  await userEvent.click(await chip());
  await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 }));
}

async function openShell(options: Parameters<typeof mount>[0] = {}) {
  const view = mount(options);
  await startOpen();
  await waitFor(() => expect(xterm?.text()).toContain("Monoptah mock shell"), { timeout: 10_000 });
  return view;
}

describe("the terminal as a tab", () => {
  it("the footer chip opens a terminal tab; a project, the passkey ticket and the desktop's check then open the shell", async () => {
    await openShell();
    expect(t.ticket).toHaveBeenCalledWith(DESKTOP_ID, DEVICE_ID, expect.any(String));
    expect(desktop.terminals.ticketVerifications()).toBe(1);
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    const tab = await screen.findByRole("tab", { name: /Terminal - Example Alpha/ });
    expect(within(tab).getByTestId("terminal-dot")).toHaveAttribute("data-state", "on");
    expect(await screen.findByTestId("terminal-count")).toHaveTextContent("1");
    await act(async () => xterm.handlers!.onData("echo hello\r"));
    await waitFor(() => expect(xterm.text()).toContain("echo hello\r\nhello\r\n$ "), { timeout: 5_000 });
    // No code step anywhere.
    expect(screen.queryByTestId("terminal-code-dialog")).toBeNull();
  });

  it("before a project is chosen the tab is called Terminal, with a grey dot", async () => {
    mount();
    await userEvent.click(await chip());
    const tab = await screen.findByRole("tab", { name: /^Terminal/ });
    expect(tab).toHaveTextContent("Terminal");
    expect(within(tab).getByTestId("terminal-dot")).toHaveAttribute("data-state", "off");
    expect(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 })).toBeInTheDocument();
  });

  it("each click of the chip opens another tab and the count follows the running shells", async () => {
    await openShell();
    await userEvent.click(screen.getByTestId("terminal-chip"));
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 }));
    await waitFor(() => expect(desktop.terminals.liveIds()).toHaveLength(2), { timeout: 10_000 });
    expect(screen.getAllByRole("tab", { name: /Terminal - Example Alpha/ })).toHaveLength(2);
    await waitFor(() => expect(screen.getByTestId("terminal-count")).toHaveTextContent("2"));
  });

  it("a refused ticket shows the error in the chooser and opens nothing", async () => {
    mount({ verify: false });
    await startOpen();
    expect(await screen.findByText(/refused the passkey check/, {}, { timeout: 10_000 })).toBeInTheDocument();
    expect(desktop.terminals.ticketVerifications()).toBe(1);
    expect(desktop.terminals.liveIds()).toEqual([]);
    expect(screen.getByRole("button", { name: "Open terminal" })).toBeInTheDocument();
  });

  it("a cancelled passkey prompt says so and never calls the desktop", async () => {
    t.ticket.mockRejectedValue(new ReauthCancelledError());
    mount();
    await startOpen();
    expect(await screen.findByText(/Passkey confirmation was cancelled/, {}, { timeout: 10_000 })).toBeInTheDocument();
    expect(desktop.terminals.ticketVerifications()).toBe(0);
  });

  it("typing exit ends the session and shows the exit text", async () => {
    await openShell();
    await act(async () => xterm.handlers!.onData("exit\r"));
    expect(await screen.findByTestId("terminal-ended", {}, { timeout: 5_000 })).toHaveTextContent("Session ended: the shell exited with code 0.");
    expect(within(screen.getByRole("tab", { name: /Terminal - Example Alpha/ })).getByTestId("terminal-dot")).toHaveAttribute("data-state", "off");
  });

  it.each([
    ["killed", "closed on the desktop"],
    ["idle", "idle for 15 minutes"],
    ["revoked", "removed from the desktop"],
    ["max", "time limit"],
  ] as const)("a shell the desktop ends (%s) shows the ended state", async (reason, text) => {
    await openShell();
    const [id] = desktop.terminals.liveIds();
    await act(async () => desktop.terminals.kill(id!, reason));
    expect(await screen.findByTestId("terminal-ended", {}, { timeout: 5_000 })).toHaveTextContent(text);
  });

  it("turning Allow web terminal off for the device ends the shell", async () => {
    await openShell();
    await act(async () => desktop.setDevice(deviceFingerprint, { terminal: false }));
    expect(await screen.findByTestId("terminal-ended", {}, { timeout: 5_000 })).toHaveTextContent("Allow web terminal was turned off");
  });

  it("closing the tab of a running shell asks first, then kills the shell and removes the tab", async () => {
    await openShell();
    const tab = await screen.findByRole("tab", { name: /Terminal - Example Alpha/ });
    await userEvent.click(within(tab.parentElement as HTMLElement).getByTestId("tab-close"));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Close this terminal?")).toBeInTheDocument();
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    await userEvent.click(within(dialog).getByRole("button", { name: "Close terminal" }));
    await waitFor(() => expect(desktop.terminals.liveIds()).toEqual([]), { timeout: 5_000 });
    await waitFor(() => expect(screen.queryByRole("tab", { name: /Terminal - Example Alpha/ })).toBeNull());
  });

  it("cancelling the confirmation keeps the tab and the shell", async () => {
    await openShell();
    const tab = await screen.findByRole("tab", { name: /Terminal - Example Alpha/ });
    await userEvent.click(within(tab.parentElement as HTMLElement).getByTestId("tab-close"));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    expect(screen.getByRole("tab", { name: /Terminal - Example Alpha/ })).toBeInTheDocument();
  });

  it("closing a tab that has no shell yet needs no confirmation", async () => {
    mount();
    await userEvent.click(await chip());
    const tab = await screen.findByRole("tab", { name: /^Terminal/ });
    await userEvent.click(within(tab.parentElement as HTMLElement).getByTestId("tab-close"));
    await waitFor(() => expect(screen.queryByRole("tab", { name: /^Terminal/ })).toBeNull());
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a reload within the grace window reattaches the tab to the same shell and shows what the desktop sends next", async () => {
    const view = await openShell();
    await act(async () => xterm.handlers!.onData("echo one\r"));
    await waitFor(() => expect(xterm.text()).toContain("one\r\n$ "), { timeout: 5_000 });
    const [id] = desktop.terminals.liveIds();
    await waitFor(() => expect(Object.keys(sessionStorage).some((key) => key.startsWith(RESUME_KEY_PREFIX))).toBe(true));
    const key = Object.keys(sessionStorage).find((name) => name.startsWith(RESUME_KEY_PREFIX)) ?? "";
    expect(JSON.parse(sessionStorage.getItem(key) ?? "{}")).toMatchObject({ terminalId: id, next: expect.any(Number) });
    // The page goes away; its state is rebuilt from localStorage (the tab) and sessionStorage (the offset), the shell lives on.
    const url = view.router.state.location.pathname;
    expect(url).toMatch(/\/t\//);
    cleanup();
    resetTabsStore();
    resetTerminalSessions();
    expect(desktop.terminals.liveIds()).toEqual([id]);
    const verifications = desktop.terminals.ticketVerifications();
    renderApp(url);
    await waitFor(() => expect(within(screen.getByRole("tab", { name: /Terminal - Example Alpha/ })).getByTestId("terminal-dot")).toHaveAttribute("data-state", "on"), { timeout: 10_000 });
    await act(async () => xterm.handlers!.onData("echo two\r"));
    await waitFor(() => expect(xterm.text()).toContain("two\r\n$ "), { timeout: 5_000 });
    expect(xterm.text()).not.toContain("echo one");
    // Reattaching by terminal id needs no new ticket.
    expect(desktop.terminals.ticketVerifications()).toBe(verifications);
    expect(desktop.terminals.liveIds()).toEqual([id]);
  });

  it("a terminal tab of a reload with no resume entry shows the chooser, not a live shell", async () => {
    const view = mount();
    await userEvent.click(await chip());
    await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 });
    const url = view.router.state.location.pathname;
    cleanup();
    resetTabsStore();
    resetTerminalSessions();
    sessionStorage.clear();
    renderApp(url);
    expect(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 })).toBeInTheDocument();
  });

  it("a read-only device has no footer chip", async () => {
    mount({ permission: "read" });
    await screen.findByTestId("app-footer", {}, { timeout: 10_000 });
    expect(screen.queryByTestId("terminal-chip")).toBeNull();
  });

  it("a desktop that does not advertise terminal (also: Allow web terminal off) has no chip, and the rail has no Terminal entry", async () => {
    mount({ terminal: false, deviceTerminal: false });
    await screen.findByTestId("app-footer", {}, { timeout: 10_000 });
    expect(screen.queryByTestId("terminal-chip")).toBeNull();
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
  });

  it("the old /terminal URL goes to the workspace home", async () => {
    const { router } = mount({ path: `/d/${DESKTOP_ID}/terminal` });
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}`), { timeout: 10_000 });
  });
});

describe("the terminal drawer on a phone", () => {
  async function openDrawer() {
    mount({ width: 390, touch: true });
    await userEvent.click(await chip());
    return screen.findByTestId("terminal-drawer");
  }

  it("the footer shows on the workspace screen and the chip opens a bottom drawer that is not modal", async () => {
    const drawer = await openDrawer();
    expect(drawer).toHaveAttribute("data-fullscreen", "false");
    expect(drawer).not.toHaveAttribute("aria-modal");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByTestId("app-shell")).toBeInTheDocument();
    expect(screen.getByTestId("terminal-chip")).toHaveAttribute("aria-pressed", "true");
  });

  it("opens a shell from the drawer chooser, shows the project with a green dot and the key row with Paste last", async () => {
    await openDrawer();
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 }));
    const keys = await screen.findByTestId("terminal-keys", {}, { timeout: 10_000 });
    expect(within(screen.getByTestId("terminal-drawer-title")).getByTestId("terminal-dot")).toHaveAttribute("data-state", "on");
    expect(screen.getByTestId("terminal-drawer-title")).toHaveTextContent("Example Alpha");
    const buttons = keys.querySelectorAll("button");
    expect(buttons[buttons.length - 1]).toHaveAccessibleName("Paste");
  });

  it("Fullscreen fills the screen and turns into Shrink; Close drawer hides it and the shell keeps running", async () => {
    await openDrawer();
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 }));
    await screen.findByTestId("terminal-keys", {}, { timeout: 10_000 });
    await userEvent.click(screen.getByRole("button", { name: "Fullscreen" }));
    expect(screen.getByTestId("terminal-drawer")).toHaveAttribute("data-fullscreen", "true");
    await userEvent.click(screen.getByRole("button", { name: "Shrink" }));
    expect(screen.getByTestId("terminal-drawer")).toHaveAttribute("data-fullscreen", "false");
    await userEvent.click(screen.getByRole("button", { name: "Close drawer" }));
    expect(screen.queryByTestId("terminal-drawer")).toBeNull();
    expect(desktop.terminals.liveIds()).toHaveLength(1);
    // Opening it again shows the same shell.
    await userEvent.click(screen.getByTestId("terminal-chip"));
    expect(await screen.findByTestId("terminal-keys")).toBeInTheDocument();
    expect(desktop.terminals.liveIds()).toHaveLength(1);
  });

  it("several terminals get a switcher in the drawer header", async () => {
    await openDrawer();
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 }));
    await screen.findByTestId("terminal-keys", {}, { timeout: 10_000 });
    expect(screen.queryByTestId("terminal-switcher")).toBeNull();
    expect(screen.queryByRole("button", { name: "End this terminal" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /show terminals/ }));
    await userEvent.click(await screen.findByRole("button", { name: "New terminal" }));
    const switcher = await screen.findByTestId("terminal-switcher");
    const tabs = within(switcher).getAllByRole("tab");
    expect(tabs).toHaveLength(2);
    expect(tabs[1]).toHaveAttribute("aria-selected", "true");
    await userEvent.click(tabs[0]!);
    expect(within(screen.getByTestId("terminal-switcher")).getAllByRole("tab")[0]).toHaveAttribute("aria-selected", "true");
  });

  it("ending a terminal from the switcher asks first and kills the shell", async () => {
    await openDrawer();
    await userEvent.click(await screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 }));
    await screen.findByTestId("terminal-keys", {}, { timeout: 10_000 });
    await userEvent.click(screen.getByRole("button", { name: /show terminals/ }));
    await userEvent.click(await screen.findByRole("button", { name: /^End / }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Close terminal" }));
    await waitFor(() => expect(desktop.terminals.liveIds()).toEqual([]), { timeout: 5_000 });
    await waitFor(() => expect(screen.queryByTestId("terminal-drawer")).toBeNull());
  });
});
