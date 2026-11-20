// The terminal page against the real link client wired to the in-process mock desktop (a fake shell with the desktop's
// code, attempts, grace and exit rules). Only xterm is replaced by a recording stand-in: jsdom has no canvas.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { KNOWN_CAPABILITIES } from "@monoptah/protocol";
import type { MockDesktop } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { clearPanels } from "@/features/layout/panels";
import { DESKTOP_ID, connectMockDesktop } from "@/features/sessions/dev/mockLink";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { AppShell } from "@/features/layout/AppShell";
import { clearSlots, registerSlot } from "@/ui/slots";
import { RESUME_KEY_PREFIX } from "./resume";
import { FakeXterm } from "./testing";
import type { ScreenHandlers } from "./xtermScreen";

const h = vi.hoisted(() => ({ screens: [] as unknown[], made: null as null | ((handlers: ScreenHandlers) => unknown) }));
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
  h.made = (handlers) => {
    xterm = new FakeXterm();
    xterm.handlers = handlers;
    return xterm;
  };
});
afterEach(() => {
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

function mount({ permission = "control", terminal = true, width = 1280, touch = false, deviceTerminal = true }: { permission?: "read" | "control"; terminal?: boolean; width?: number; touch?: boolean; deviceTerminal?: boolean } = {}) {
  viewport(width, touch);
  const link = connectMockDesktop({
    permission,
    capabilities: terminal ? [...KNOWN_CAPABILITIES] : KNOWN_CAPABILITIES.filter((name) => name !== "terminal"),
    seed: (mock) => {
      desktop = mock;
    },
  });
  desktop = link.desktop;
  deviceFingerprint = [...desktop.devices.keys()][0] ?? "";
  if (!deviceTerminal) desktop.setDevice(deviceFingerprint, { terminal: false });
  return renderApp();
}

/** Renders the app on the terminal route against the link that is already registered (a "reload" renders it again). */
function renderApp() {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}/terminal`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

async function openChooser() {
  return screen.findByRole("button", { name: "Open terminal" }, { timeout: 10_000 });
}

/** Clicks Open, waits for the code dialog and returns the code the desktop "shows". */
async function startOpen() {
  await userEvent.click(await openChooser());
  const dialog = await screen.findByTestId("terminal-code-dialog", {}, { timeout: 10_000 });
  await waitFor(() => expect(desktop.terminals.lastCode()).not.toBeNull());
  return { dialog, code: desktop.terminals.lastCode() ?? "" };
}

async function enter(code: string) {
  const input = await screen.findByTestId("terminal-code-input");
  await userEvent.clear(input);
  await userEvent.type(input, code);
  await userEvent.click(screen.getByTestId("terminal-code-submit"));
}

async function openShell() {
  mountDefault();
  const { code } = await startOpen();
  await enter(code);
  await waitFor(() => expect(screen.getByTestId("terminal-status")).toHaveTextContent("Connected"), { timeout: 10_000 });
}
const mountDefault = () => mount();

describe("the terminal page", () => {
  it("opens a shell with the code, shows the desktop's banner and echoes what is typed", async () => {
    await openShell();
    expect(screen.getByTestId("terminal-cwd")).toHaveTextContent("Example Alpha");
    await waitFor(() => expect(xterm.text()).toContain("Monoptah mock shell in Example Alpha"));
    await act(async () => xterm.handlers!.onData("echo hello\r"));
    await waitFor(() => expect(xterm.text()).toContain("echo hello\r\nhello\r\n$ "), { timeout: 5_000 });
    expect(desktop.terminals.liveIds()).toHaveLength(1);
  });

  it("a wrong code asks again with the tries left; the right one then opens", async () => {
    mount();
    const { code } = await startOpen();
    await enter(code === "ZZZZZZ" ? "YYYYYY" : "ZZZZZZ");
    expect(await screen.findByText("That code is not right. 2 tries left.")).toBeInTheDocument();
    expect(screen.getByTestId("terminal-code-dialog")).toBeInTheDocument();
    await enter(code.toLowerCase());
    await waitFor(() => expect(screen.getByTestId("terminal-status")).toHaveTextContent("Connected"), { timeout: 10_000 });
    expect(screen.queryByTestId("terminal-code-dialog")).toBeNull();
  });

  it("three wrong codes use the challenge up: a new code is asked for and the old one no longer works", async () => {
    mount();
    const { code: first } = await startOpen();
    const wrong = first === "ZZZZZZ" ? "YYYYYY" : "ZZZZZZ";
    await enter(wrong);
    await screen.findByText("That code is not right. 2 tries left.");
    await enter(wrong);
    await screen.findByText("That code is not right. 1 try left.");
    await enter(wrong);
    expect(await screen.findByText(/A new code is now shown on your desktop/)).toBeInTheDocument();
    await waitFor(() => expect(desktop.terminals.lastCode()).not.toBeNull());
    const second = desktop.terminals.lastCode() ?? "";
    expect(screen.getByTestId("terminal-code-input")).toHaveValue("");
    await enter(second);
    await waitFor(() => expect(screen.getByTestId("terminal-status")).toHaveTextContent("Connected"), { timeout: 10_000 });
  });

  it("typing exit ends the session and shows the exit text", async () => {
    await openShell();
    await act(async () => xterm.handlers!.onData("exit\r"));
    expect(await screen.findByTestId("terminal-ended", {}, { timeout: 5_000 })).toHaveTextContent("Session ended: the shell exited with code 0.");
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
    expect(sessionStorage.getItem(`${RESUME_KEY_PREFIX}${DESKTOP_ID}`)).toBeNull();
  });

  it("turning Allow web terminal off for the device ends the shell and removes the capability", async () => {
    await openShell();
    await act(async () => desktop.setDevice(deviceFingerprint, { terminal: false }));
    expect(await screen.findByTestId("terminal-ended", {}, { timeout: 5_000 })).toHaveTextContent("Allow web terminal was turned off");
  });

  it("Close terminal kills the shell on the desktop", async () => {
    await openShell();
    await userEvent.click(screen.getByRole("button", { name: "Close terminal" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Close terminal" }));
    expect(await screen.findByTestId("terminal-ended", {}, { timeout: 5_000 })).toHaveTextContent("you closed the terminal");
    expect(desktop.terminals.liveIds()).toEqual([]);
  });

  it("a burst of output is drawn in full, slice by slice", async () => {
    await openShell();
    await act(async () => xterm.handlers!.onData("flood\r"));
    await waitFor(() => expect(xterm.text()).toContain("flood done"), { timeout: 20_000 });
    expect(xterm.text().match(/flood line/g)?.length).toBe(3000);
    expect(Math.max(...xterm.writes.map((chunk) => chunk.length))).toBeLessThanOrEqual(16 * 1024);
  });

  it("a reload within the grace window reattaches to the same shell and shows what the desktop sends next", async () => {
    await openShell();
    await act(async () => xterm.handlers!.onData("echo one\r"));
    await waitFor(() => expect(xterm.text()).toContain("one\r\n$ "), { timeout: 5_000 });
    const [id] = desktop.terminals.liveIds();
    await waitFor(() => expect(JSON.parse(sessionStorage.getItem(`${RESUME_KEY_PREFIX}${DESKTOP_ID}`) ?? "{}")).toMatchObject({ terminalId: id, next: expect.any(Number) }));
    // The page goes away; its state is rebuilt from sessionStorage, the shell lives on in the desktop.
    cleanup();
    expect(desktop.terminals.liveIds()).toEqual([id]);
    renderApp();
    await waitFor(() => expect(screen.getByTestId("terminal-status")).toHaveTextContent("Connected"), { timeout: 10_000 });
    await act(async () => xterm.handlers!.onData("echo two\r"));
    await waitFor(() => expect(xterm.text()).toContain("two\r\n$ "), { timeout: 5_000 });
    // Earlier output is not kept anywhere: the new screen starts after what the first one showed.
    expect(xterm.text()).not.toContain("echo one");
    expect(desktop.terminals.liveIds()).toEqual([id]);
  });

  it("a read-only device sees no entry and an explanation on the page", async () => {
    mount({ permission: "read", terminal: true });
    expect(await screen.findByTestId("terminal-disabled")).toHaveTextContent("A terminal needs control access");
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
  });

  it("a Control device with Allow web terminal off sees the explanation and no rail entry", async () => {
    mount({ deviceTerminal: false });
    expect(await screen.findByTestId("terminal-disabled")).toHaveTextContent("Turn on Allow web terminal for this device in Monoptah on your desktop");
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
  });

  it("a desktop that does not advertise terminal has no entry and says the same", async () => {
    mount({ terminal: false });
    expect(await screen.findByTestId("terminal-disabled")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull();
  });

  it("with the capability the rail has a Terminal entry that opens the page", async () => {
    registerSlot("app-shell", AppShell);
    mount();
    expect(await screen.findByRole("link", { name: "Terminal" }, { timeout: 10_000 })).toHaveAttribute("href", `/d/${DESKTOP_ID}/terminal`);
  });

  it("on a phone the key row shows once the shell runs", async () => {
    mount({ width: 390, touch: true });
    await userEvent.click(await openChooser());
    const { code } = { code: await waitFor(() => desktop.terminals.lastCode() ?? Promise.reject(new Error("no code"))) };
    await enter(code);
    expect(await screen.findByTestId("terminal-keys", {}, { timeout: 10_000 })).toBeInTheDocument();
  });
});
