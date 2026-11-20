import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fromBase64 } from "./input";
import { TerminalSession } from "./session";
import { FakeLink, fakeScreenFactory, readResult, type FakeXterm } from "./testing";
import { TerminalPane } from "./TerminalPane";

vi.mock("./TerminalChooser", () => ({ TerminalChooser: () => <div data-testid="chooser-stub" /> }));

const ENTRY = { terminalId: "term-1", next: 0, cwd: "Example Alpha/src", idleTimeoutMs: 900_000, maxDurationMs: 28_800_000, openedAt: Date.now() };

function coarse(on: boolean) {
  window.matchMedia = ((query: string) => ({ matches: on && query.includes("coarse"), media: query, onchange: null, addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false })) as unknown as typeof window.matchMedia;
}

/** A connection over a FakeLink, as far as the session needs one. */
function connectionOf(link: FakeLink) {
  const listeners = new Set<() => void>();
  link.subscribeReady(() => listeners.forEach((listener) => listener()));
  return {
    request: (method: string, params: unknown, options?: { signal?: AbortSignal }) => link.request(method as never, params as never, options),
    store: { getState: () => ({ status: link.ready ? "ready" : "closed" }), subscribe: (listener: () => void) => (listeners.add(listener), () => listeners.delete(listener)) },
  };
}

function rig({ variant = "tab", touch = false, resumed = true }: { variant?: "tab" | "drawer"; touch?: boolean; resumed?: boolean } = {}) {
  coarse(touch);
  const link = new FakeLink();
  const store = { entry: resumed ? ENTRY : null, load() { return this.entry; }, save() {}, clear() { this.entry = null; } };
  const session = new TerminalSession("desk", `t${Math.random().toString(36).slice(2, 8)}`, connectionOf(link) as never, store);
  const factory = fakeScreenFactory();
  const view = render(<TerminalPane session={session} variant={variant} createScreen={factory.create} />);
  return { link, session, factory, view };
}

const writes = (link: FakeLink): string[] => link.all("terminal.write").map((call) => new TextDecoder().decode(fromBase64(String(call.params["data"]))!));
const flush = () => act(async () => { await Promise.resolve(); });

beforeEach(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("TerminalPane", () => {
  it("moves the screen element in, creates the screen once, and the read loop starts only after it exists", async () => {
    const { link, session, factory } = rig();
    expect(screen.getByTestId("terminal-screen")).toContainElement(session.element);
    expect(link.open("terminal.read")).toHaveLength(0);
    await flush();
    expect(link.open("terminal.read")).toHaveLength(1);
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    expect(factory.xterm.text()).toBe("$ ");
  });

  it("has no status line and no Copy, Paste or Close toolbar", async () => {
    rig();
    await flush();
    for (const name of ["Copy", "Paste", "Close terminal"]) expect(screen.queryByRole("button", { name })).toBeNull();
    expect(screen.queryByTestId("terminal-status")).toBeNull();
    expect(screen.queryByTestId("terminal-limits")).toBeNull();
  });

  it("typed keys go through the controller's write path", async () => {
    const { link, factory } = rig();
    await flush();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    await act(async () => factory.xterm.handlers!.onData("ls\r"));
    expect(writes(link)).toEqual(["ls\r"]);
  });

  it("keeps the screen alive when the pane unmounts and puts the same element back on remount, and never closes the shell", async () => {
    const { link, session, factory, view } = rig();
    await flush();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    view.unmount();
    expect(factory.xterm.disposed).toBe(false);
    expect(session.element.isConnected).toBe(false);
    expect(link.all("terminal.close")).toHaveLength(0);
    render(<TerminalPane session={session} variant="tab" createScreen={factory.create} />);
    expect(screen.getByTestId("terminal-screen")).toContainElement(session.element);
    expect(factory.xterm.disposed).toBe(false);
    // The screen was made once.
    expect(session.screen).toBe(factory.xterm);
  });

  it("shows why a session ended and a New terminal button that goes back to the chooser", async () => {
    const { link, session } = rig();
    await flush();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "", { reason: "idle" })));
    expect(screen.getByTestId("terminal-ended")).toHaveTextContent("Session ended: the terminal was idle for 15 minutes.");
    await userEvent.click(screen.getByRole("button", { name: "New terminal" }));
    expect(screen.getByTestId("chooser-stub")).toBeInTheDocument();
    expect(session.target).toBeNull();
  });

  it("the context menu copies the selection and pastes through the screen, as typed input", async () => {
    const { link, factory } = rig();
    await flush();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText, readText: vi.fn().mockResolvedValue("echo pasted") } });
    const xterm: FakeXterm = factory.xterm;
    xterm.selection = "npm test";
    await act(async () => {
      screen.getByTestId("terminal-screen").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    });
    await userEvent.click(await screen.findByRole("menuitem", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("npm test");
    await act(async () => {
      screen.getByTestId("terminal-screen").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
    });
    await userEvent.click(await screen.findByRole("menuitem", { name: "Paste" }));
    expect(xterm.pasted).toEqual(["echo pasted"]);
    expect(writes(link)).toEqual(["echo pasted"]);
  });

  it("says when the clipboard is not allowed", async () => {
    const { link } = rig({ variant: "drawer" });
    await flush();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(), readText: vi.fn().mockRejectedValue(new Error("denied")) } });
    await userEvent.click(screen.getByRole("button", { name: "Paste" }));
    expect(await screen.findByText(/Paste is not allowed here/)).toBeInTheDocument();
  });

  it("a tab shows the key row on touch screens only; the drawer always does, with Paste last, and its keys share the write path with the sticky Ctrl", async () => {
    rig();
    await flush();
    expect(screen.queryByTestId("terminal-keys")).toBeNull();
    cleanup();
    const { link, factory } = rig({ variant: "drawer" });
    await flush();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    const keys = screen.getByTestId("terminal-keys");
    expect(keys.querySelectorAll("button")[keys.querySelectorAll("button").length - 1]).toHaveAccessibleName("Paste");
    await userEvent.click(screen.getByRole("button", { name: "Esc" }));
    expect(writes(link)).toEqual(["\x1b"]);
    for (const call of link.open("terminal.write")) await act(async () => call.resolve({}));
    await userEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "true");
    await act(async () => factory.xterm.handlers!.onData("c"));
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "false");
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 80));
    });
    expect(writes(link).at(-1)).toBe("\x03");
  });

  it("a tab on a touch screen shows the key row too", async () => {
    rig({ touch: true });
    await flush();
    expect(screen.getByTestId("terminal-keys")).toBeInTheDocument();
  });

  it("shows a load error when the screen cannot be created", async () => {
    coarse(false);
    const link = new FakeLink();
    const session = new TerminalSession("desk", "tload", connectionOf(link) as never, { load: () => ENTRY, save() {}, clear() {} });
    render(<TerminalPane session={session} variant="tab" createScreen={() => Promise.reject(new Error("chunk failed"))} />);
    expect(await screen.findByText(/could not be loaded/)).toBeInTheDocument();
  });
});
