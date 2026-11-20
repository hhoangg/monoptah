import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TerminalController } from "./controller";
import { fromBase64 } from "./input";
import { FakeLink, fakeScreenFactory, readResult } from "./testing";
import { TerminalView } from "./TerminalView";

const ENTRY = { terminalId: "term-1", next: 0, cwd: "Example Alpha/src", idleTimeoutMs: 900_000, maxDurationMs: 28_800_000, openedAt: Date.now() };

function touchLayout(on: boolean) {
  window.matchMedia = ((query: string) => ({ matches: on, media: query, onchange: null, addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false })) as unknown as typeof window.matchMedia;
}

async function mount({ touch = false } = {}) {
  touchLayout(touch);
  const link = new FakeLink();
  const controller = new TerminalController({ link });
  const factory = fakeScreenFactory();
  controller.resume(ENTRY);
  const onScreen = vi.fn();
  const onLeave = vi.fn();
  render(<TerminalView controller={controller} onScreen={onScreen} onLeave={onLeave} createScreen={factory.create} />);
  await act(async () => {
    await Promise.resolve();
  });
  return { link, controller, factory, onScreen, onLeave };
}

const writes = (link: FakeLink): string[] => link.all("terminal.write").map((call) => new TextDecoder().decode(fromBase64(String(call.params["data"]))!));

beforeEach(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("TerminalView", () => {
  it("attaches the screen to the controller once and tells the page, and shows where the shell runs and its limits", async () => {
    const { onScreen, link, factory } = await mount();
    expect(onScreen).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("terminal-cwd")).toHaveTextContent("Example Alpha/src");
    expect(screen.getByTestId("terminal-limits")).toHaveTextContent("Closes after 15 min without typing");
    // The read loop started only now that a screen exists.
    expect(link.open("terminal.read")).toHaveLength(1);
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    expect(factory.xterm.text()).toBe("$ ");
    expect(screen.getByTestId("terminal-status")).toHaveTextContent("Connected");
  });

  it("typed keys go through the controller's write path", async () => {
    const { link, factory } = await mount();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    await act(async () => factory.xterm.handlers!.onData("ls\r"));
    expect(writes(link)).toEqual(["ls\r"]);
  });

  it("forwards a new window size to the controller", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { link, factory } = await mount();
      await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
      factory.xterm.handlers!.onResize(100, 30);
      await act(async () => vi.advanceTimersByTimeAsync(300));
      expect(link.all("terminal.resize")[0]?.params).toEqual({ terminalId: "term-1", cols: 100, rows: 30 });
    } finally {
      vi.useRealTimers();
    }
  });

  it("Close terminal asks first, then closes the shell on the desktop and shows the ended state", async () => {
    const { link } = await mount();
    await userEvent.click(screen.getByRole("button", { name: "Close terminal" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Close this terminal?")).toBeInTheDocument();
    expect(link.all("terminal.close")).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole("button", { name: "Close terminal" }));
    await act(async () => link.open("terminal.close")[0]!.resolve({}));
    expect(await screen.findByTestId("terminal-ended")).toHaveTextContent("Session ended: you closed the terminal.");
  });

  it("shows why a session ended, disables the buttons and offers a new terminal", async () => {
    const { link, onLeave } = await mount();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "", { reason: "idle" })));
    expect(screen.getByTestId("terminal-ended")).toHaveTextContent("Session ended: the terminal was idle for 15 minutes.");
    expect(screen.getByRole("button", { name: "Close terminal" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Open a new terminal" }));
    expect(onLeave).toHaveBeenCalled();
  });

  it("Copy writes the selection to the clipboard; with nothing selected it says so", async () => {
    const { factory, link } = await mount();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText, readText: vi.fn() } });
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).not.toHaveBeenCalled();
    expect(screen.getByText("Select some text first")).toBeInTheDocument();
    factory.xterm.selection = "npm test";
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("npm test");
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });

  it("Paste reads the clipboard and pastes it through the screen, so it is typed input", async () => {
    const { factory, link } = await mount();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(), readText: vi.fn().mockResolvedValue("echo pasted") } });
    await userEvent.click(screen.getByRole("button", { name: "Paste" }));
    expect(factory.xterm.pasted).toEqual(["echo pasted"]);
    expect(writes(link)).toEqual(["echo pasted"]);
  });

  it("says when the clipboard is not allowed", async () => {
    const { link } = await mount();
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(), readText: vi.fn().mockRejectedValue(new Error("denied")) } });
    await userEvent.click(screen.getByRole("button", { name: "Paste" }));
    expect(await screen.findByText(/Paste is not allowed here/)).toBeInTheDocument();
  });

  it("shows the key row on a touch layout only, and its keys share the write path with the sticky Ctrl", async () => {
    const desktop = await mount();
    expect(screen.queryByTestId("terminal-keys")).toBeNull();
    desktop.controller.dispose();
    cleanup();
    const { link, factory } = await mount({ touch: true });
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    expect(screen.getByTestId("terminal-keys")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Esc" }));
    expect(writes(link)).toEqual(["\x1b"]);
    for (const call of link.open("terminal.write")) await act(async () => call.resolve({}));
    // Ctrl, then a letter typed on the soft keyboard (xterm's data event): ^C.
    await userEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "true");
    await act(async () => factory.xterm.handlers!.onData("c"));
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "false");
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 80));
    });
    expect(writes(link).at(-1)).toBe("\x03");
  });

  it("uses the application form of the arrow keys when the program switched to it", async () => {
    const { link, factory } = await mount({ touch: true });
    factory.xterm.application = true;
    await act(async () => link.open("terminal.read")[0]!.resolve(readResult(0, "$ ")));
    await userEvent.click(screen.getByRole("button", { name: "Up arrow" }));
    expect(writes(link)).toEqual(["\x1bOA"]);
  });

  it("disposes the screen on unmount and never closes the shell", async () => {
    touchLayout(false);
    const link = new FakeLink();
    const controller = new TerminalController({ link });
    const factory = fakeScreenFactory();
    controller.resume(ENTRY);
    const view = render(<TerminalView controller={controller} onScreen={() => undefined} onLeave={() => undefined} createScreen={factory.create} />);
    await act(async () => {
      await Promise.resolve();
    });
    view.unmount();
    expect(factory.xterm.disposed).toBe(true);
    expect(link.all("terminal.close")).toHaveLength(0);
  });

  it("shows a load error when the screen cannot be created", async () => {
    touchLayout(false);
    const controller = new TerminalController({ link: new FakeLink() });
    render(<TerminalView controller={controller} onScreen={() => undefined} onLeave={() => undefined} createScreen={() => Promise.reject(new Error("chunk failed"))} />);
    expect(await screen.findByText(/could not be loaded/)).toBeInTheDocument();
  });
});
