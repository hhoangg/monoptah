// @vitest-environment happy-dom
import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";

const pty = vi.hoisted(() => ({
  spawnPty: vi.fn(async () => {}),
  killPty: vi.fn(async () => {}),
  resizePty: vi.fn(async () => {}),
  writePty: vi.fn(async () => {}),
  subscribePty: vi.fn(() => () => {}),
  holdPtyBridge: vi.fn(() => () => {}),
  // Rejects like the backend does for a PTY that is not running.
  getPtyStatus: vi.fn(
    async (_id?: string): Promise<{ foreground: string | null }> => {
      throw new Error("Terminal is not running");
    },
  ),
}));
vi.mock("../../../platform/tauri/pty", () => pty);
const xterm = vi.hoisted(() => ({
  options: [] as { fontFamily?: string }[],
  titleListeners: [] as ((title: string) => void)[],
  dataListeners: [] as ((data: string) => void)[],
}));
// Null like a terminal that has not been laid out; a test that needs the first
// fit to happen gives it a size.
const layout = vi.hoisted(() => ({
  fitTerminal: vi.fn((): { cols: number; rows: number } | null => null),
}));
vi.mock("../model/terminalLayout", () => ({
  fitTerminal: layout.fitTerminal,
  applyTerminalChrome: () => {},
}));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    constructor(options: { fontFamily?: string }) {
      xterm.options.push(options);
    }
    cols = 80;
    rows = 24;
    options = {};
    parser = { registerOscHandler: () => ({ dispose() {} }) };
    buffer = {
      active: { type: "normal" },
      onBufferChange: () => ({ dispose() {} }),
    };
    open() {}
    focus() {}
    dispose() {}
    writeln() {}
    write() {}
    onData(listener: (data: string) => void) {
      xterm.dataListeners.push(listener);
      return { dispose() {} };
    }
    onRender() {
      return { dispose() {} };
    }
    attachCustomKeyEventHandler() {}
    attachCustomWheelEventHandler() {}
    onTitleChange(listener: (title: string) => void) {
      xterm.titleListeners.push(listener);
      return { dispose() {} };
    }
  },
}));
import { TerminalView } from "./TerminalView";
import { markPtyHandoff, settlePtyHandoffs } from "../model/ptyHandoff";

afterEach(() => {
  settlePtyHandoffs();
  layout.fitTerminal.mockReset();
  pty.getPtyStatus.mockReset();
  pty.resizePty.mockReset();
  xterm.options.length = 0;
  xterm.titleListeners.length = 0;
  xterm.dataListeners.length = 0;
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

function setup() {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  const host = document.createElement("div");
  document.body.appendChild(host);
  return { host, root: createRoot(host) };
}

it("does not let StrictMode cleanup kill the replacement shell", async () => {
  const { host, root } = setup();
  try {
    await act(async () => {
      root.render(
        createElement(
          StrictMode,
          null,
          createElement(TerminalView, {
            id: "same-id",
            cwd: "/tmp",
            active: true,
          }),
        ),
      );
    });
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    expect(pty.subscribePty).toHaveBeenCalledTimes(1);
    expect(pty.killPty).not.toHaveBeenCalled();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
  expect(pty.killPty).toHaveBeenCalledTimes(1);
});

it("waits for a pending same-id startup and cleanup before starting again", async () => {
  const { host, root } = setup();
  const operations: string[] = [];
  let releaseSpawn!: () => void;
  pty.subscribePty.mockImplementation(() => {
    operations.push("subscribe");
    return () => {
      operations.push("unsubscribe");
    };
  });
  pty.spawnPty
    .mockImplementationOnce(() => {
      operations.push("spawn old");
      return new Promise<void>((resolve) => {
        releaseSpawn = resolve;
      });
    })
    .mockImplementationOnce(async () => {
      operations.push("spawn replacement");
    });
  pty.killPty.mockImplementation(async () => {
    operations.push("kill");
  });
  // A new `key` remounts a fresh instance with the same PTY id, as moving a
  // terminal between the dock and a file pane does.
  const view = (key: string) =>
    createElement(TerminalView, {
      key,
      id: "moved",
      cwd: "/tmp",
      active: true,
    });
  try {
    await act(async () => {
      root.render(view("dock"));
    });
    await act(async () => {
      root.render(view("pane"));
    });
    expect(operations).toEqual(["subscribe", "spawn old"]);
    await act(async () => {
      releaseSpawn();
    });
    expect(operations).toEqual([
      "subscribe",
      "spawn old",
      "unsubscribe",
      "kill",
      "subscribe",
      "spawn replacement",
    ]);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("does not hold a different terminal behind another one's teardown", async () => {
  const { host, root } = setup();
  pty.spawnPty.mockImplementationOnce(() => new Promise<void>(() => {}));
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, { id: "first", cwd: "/tmp", active: true }),
      );
    });
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "second",
          cwd: "/tmp",
          active: true,
        }),
      );
    });
    expect(pty.spawnPty).toHaveBeenCalledTimes(2);
    expect(pty.spawnPty).toHaveBeenLastCalledWith("second", "/tmp", 80, 24);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("uses the terminal-specific font stack", async () => {
  const { host, root } = setup();
  const stack = '"Test Nerd Font", monospace';
  document.documentElement.style.setProperty("--font-terminal", stack);
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, { id: "font", cwd: "/tmp", active: true }),
      );
    });
    expect(xterm.options[0]?.fontFamily).toBe(stack);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
    document.documentElement.style.removeProperty("--font-terminal");
  }
});

it("spawns the plain shell with exactly four arguments when there is no launch", async () => {
  const { host, root } = setup();
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, { id: "shell", cwd: "/tmp", active: true }),
      );
    });
    expect(pty.spawnPty.mock.calls).toEqual([["shell", "/tmp", 80, 24]]);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("passes a launch through to the spawn", async () => {
  const { host, root } = setup();
  const launch = {
    program: "/opt/bin/claude",
    args: ["--session-id", "abc"],
    providerAccount: { provider: "claude" as const, id: "work" },
  };
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "tui",
          cwd: "/tmp",
          active: true,
          launch,
        }),
      );
    });
    expect(pty.spawnPty.mock.calls).toEqual([["tui", "/tmp", 80, 24, launch]]);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("does not respawn when only the launch object changes", async () => {
  const { host, root } = setup();
  const render = () =>
    createElement(TerminalView, {
      id: "stable",
      cwd: "/tmp",
      active: true,
      launch: { program: "claude", args: ["--resume", "abc"] },
    });
  try {
    await act(async () => {
      root.render(render());
    });
    await act(async () => {
      root.render(render());
    });
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    expect(pty.killPty).not.toHaveBeenCalled();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("reports the process exit to the caller", async () => {
  const { host, root } = setup();
  const onExit = vi.fn();
  let reportExit!: (code: number | null) => void;
  pty.subscribePty.mockImplementationOnce(((
    _id: string,
    _onData: unknown,
    onExitEvent: (code: number | null) => void,
  ) => {
    reportExit = onExitEvent;
    return () => {};
  }) as never);
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "exits",
          cwd: "/tmp",
          active: true,
          onExit,
          captureExitOutput: true,
        }),
      );
    });
    expect(onExit).not.toHaveBeenCalled();
    await act(async () => {
      reportExit(2);
    });
    expect(onExit).toHaveBeenCalledExactlyOnceWith({ code: 2, output: "" });
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("reports a process that could not start as an error with no code", async () => {
  const { host, root } = setup();
  const onExit = vi.fn();
  pty.spawnPty.mockRejectedValueOnce(new Error("Failed to start claude"));
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "missing",
          cwd: "/tmp",
          active: true,
          onExit,
        }),
      );
    });
    expect(onExit).toHaveBeenCalledExactlyOnceWith({
      code: null,
      error: "Failed to start claude",
    });
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("forwards the program's window title only to a view that asked for it", async () => {
  const { host, root } = setup();
  const onTitleChange = vi.fn();
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, { id: "plain", cwd: "/tmp", active: true }),
      );
    });
    expect(xterm.titleListeners).toHaveLength(0);
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          key: "titled",
          id: "titled",
          cwd: "/tmp",
          active: true,
          onTitleChange,
        }),
      );
    });
    expect(xterm.titleListeners).toHaveLength(1);
    xterm.titleListeners[0]("Claude Code");
    expect(onTitleChange).toHaveBeenCalledExactlyOnceWith("Claude Code");
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("hands the caller the output written before the process exited", async () => {
  const { host, root } = setup();
  const onExit = vi.fn();
  let write!: (data: Uint8Array) => void;
  let reportExit!: (code: number | null) => void;
  pty.subscribePty.mockImplementationOnce(((
    _id: string,
    onData: (data: Uint8Array) => void,
    onExitEvent: (code: number | null) => void,
  ) => {
    write = onData;
    reportExit = onExitEvent;
    return () => {};
  }) as never);
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "output",
          cwd: "/tmp",
          active: true,
          onExit,
          captureExitOutput: true,
        }),
      );
    });
    const encode = (text: string) => new TextEncoder().encode(text);
    await act(async () => {
      write(encode("No conversation "));
      write(encode("found\r\n"));
      reportExit(1);
    });
    expect(onExit).toHaveBeenCalledExactlyOnceWith({
      code: 1,
      output: "No conversation found\r\n",
    });
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

async function exitWith(captureExitOutput: boolean) {
  const { host, root } = setup();
  let onData!: (data: Uint8Array) => void;
  let onExit!: (code: number | null) => void;
  pty.subscribePty.mockImplementationOnce(((
    _id: string,
    data: (data: Uint8Array) => void,
    exit: (code: number | null) => void,
  ) => {
    onData = data;
    onExit = exit;
    return () => {};
  }) as never);
  const exits: { code: number | null; output?: string }[] = [];
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "exit-id",
          cwd: "/tmp",
          active: true,
          onExit: (exit) => exits.push(exit),
          captureExitOutput,
        }),
      );
    });
    await act(async () => {
      onData(new TextEncoder().encode("bye"));
      onExit(0);
    });
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
  return exits;
}

it("reports the output tail only to a view that opted in", async () => {
  expect(await exitWith(true)).toEqual([{ code: 0, output: "bye" }]);
});

it("does not collect output for a view that only wants the exit", async () => {
  const exits = await exitWith(false);
  expect(exits).toEqual([{ code: 0 }]);
  expect(exits[0].output).toBeUndefined();
});

async function mountThenUnmount(id: string, onSpawn?: () => void) {
  const { host, root } = setup();
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, { id, cwd: "/tmp", active: true, onSpawn }),
      );
    });
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
}

const alive = async () => ({ foreground: null });

/** Lets the first animation frame run, where a terminal is first fitted. */
async function nextFrame() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 40));
  });
}

const sized = (id: string, key: string, extra: object = {}) =>
  createElement(TerminalView, {
    key,
    id,
    cwd: "/tmp",
    active: true,
    ...extra,
  });

it("leaves the process running when the unmount is a marked handoff", async () => {
  markPtyHandoff(["moving"]);
  await mountThenUnmount("moving");
  expect(pty.killPty).not.toHaveBeenCalled();
});

it("still kills the process on an unmarked unmount", async () => {
  markPtyHandoff(["other"]);
  await mountThenUnmount("closing");
  expect(pty.killPty).toHaveBeenCalledExactlyOnceWith("closing");
});

it("does not let a mark that no view consumed spare a later ordinary close", async () => {
  // Marked while no view was mounted for it (still loading), then the commit
  // that carried the move settled. A real close afterwards must still kill.
  markPtyHandoff(["late"]);
  settlePtyHandoffs();
  await mountThenUnmount("late");
  expect(pty.killPty).toHaveBeenCalledExactlyOnceWith("late");
});

it("attaches to the live process of a pane that was handed off, and repaints at the fitted size", async () => {
  const { host, root } = setup();
  const onSpawn = vi.fn();
  const steps: string[] = [];
  layout.fitTerminal.mockImplementation(() => {
    steps.push("fit");
    return { cols: 80, rows: 24 };
  });
  pty.resizePty.mockImplementation(async (_id: string, cols: number, rows: number) => {
    steps.push(`resize ${cols}x${rows}`);
  });
  pty.getPtyStatus.mockImplementationOnce(alive);
  try {
    await act(async () => {
      root.render(sized("live", "old"));
    });
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    steps.length = 0;
    markPtyHandoff(["live"]);
    await act(async () => {
      root.render(
        sized("live", "new", {
          launch: { program: "claude", args: ["--resume", "x"] },
          onSpawn,
        }),
      );
    });
    await nextFrame();
    expect(pty.killPty).not.toHaveBeenCalled();
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    expect(pty.getPtyStatus).toHaveBeenCalledWith("live");
    // The step away from the size and back both come after the first fit.
    const fitAt = steps.indexOf("fit");
    expect(fitAt).toBe(0);
    expect(steps.slice(1, 3)).toEqual(["resize 80x23", "resize 80x24"]);
    expect(onSpawn).not.toHaveBeenCalled();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("never attaches to a live process it was not handed (regression guard)", async () => {
  // Another window, a reload, a restore: the PTY may well be alive, but this
  // view does not own it. Attaching would share and resize someone else's
  // terminal, so such a mount spawns exactly as it always did.
  const { host, root } = setup();
  const onSpawn = vi.fn();
  pty.getPtyStatus.mockImplementation(alive);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(
        createElement(TerminalView, {
          id: "foreign",
          cwd: "/tmp",
          active: true,
          onSpawn,
        }),
      );
    });
    await nextFrame();
    expect(pty.spawnPty).toHaveBeenCalledExactlyOnceWith(
      "foreign",
      "/tmp",
      80,
      24,
    );
    expect(onSpawn).toHaveBeenCalledTimes(1);
    expect(pty.resizePty).not.toHaveBeenCalledWith("foreign", 80, 23);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("does not attach when the handoff mark went unused, even if the process is alive", async () => {
  markPtyHandoff(["unused"]);
  settlePtyHandoffs();
  pty.getPtyStatus.mockImplementation(alive);
  await mountThenUnmount("unused");
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
});

it("spawns as before when no process is alive", async () => {
  const onSpawn = vi.fn();
  await mountThenUnmount("fresh", onSpawn);
  expect(pty.spawnPty).toHaveBeenCalledExactlyOnceWith("fresh", "/tmp", 80, 24);
  expect(onSpawn).toHaveBeenCalledTimes(1);
});

async function attachAfterHandoff(
  status: () => Promise<{ foreground: string | null }>,
) {
  const { host, root } = setup();
  const onSpawn = vi.fn();
  const onExit = vi.fn();
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("raced", "old"));
    });
    pty.spawnPty.mockClear();
    markPtyHandoff(["raced"]);
    pty.getPtyStatus.mockImplementationOnce(status);
    await act(async () => {
      root.render(sized("raced", "new", { onSpawn, onExit }));
    });
    await nextFrame();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
  return { onSpawn, onExit };
}

it("does not spawn a shell when the process died before the attach, and says so", async () => {
  const { onSpawn, onExit } = await attachAfterHandoff(async () => {
    throw new Error("Terminal is not running");
  });
  expect(pty.spawnPty).not.toHaveBeenCalled();
  expect(onSpawn).not.toHaveBeenCalled();
  expect(onExit).toHaveBeenCalledTimes(1);
  expect(onExit.mock.calls[0][0].code).toBeNull();
  expect(onExit.mock.calls[0][0].error).toContain("Could not reattach");
});

it("does not spawn over a live process when the status check fails transiently", async () => {
  const { onSpawn, onExit } = await attachAfterHandoff(async () => {
    throw "ipc channel closed";
  });
  expect(pty.spawnPty).not.toHaveBeenCalled();
  expect(onSpawn).not.toHaveBeenCalled();
  expect(onExit.mock.calls[0][0].error).toContain("ipc channel closed");
});

it("keeps quiet about a failed attach once the exit itself was reported", async () => {
  const { host, root } = setup();
  const onExit = vi.fn();
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("gone", "old"));
    });
    pty.spawnPty.mockClear();
    // The buffered exit is replayed as the view subscribes, before the status
    // check can say the process is gone.
    pty.subscribePty.mockImplementationOnce(((
      _id: string,
      _onData: unknown,
      onExitEvent: (code: number | null) => void,
    ) => {
      onExitEvent(0);
      return () => {};
    }) as never);
    markPtyHandoff(["gone"]);
    await act(async () => {
      root.render(sized("gone", "new", { onExit }));
    });
    await nextFrame();
    expect(onExit).toHaveBeenCalledExactlyOnceWith({ code: 0 });
    expect(pty.spawnPty).not.toHaveBeenCalled();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("makes the next view wait for a spawn the handed-off view still has in flight", async () => {
  const { host, root } = setup();
  const steps: string[] = [];
  let finishSpawn!: () => void;
  pty.spawnPty.mockImplementationOnce(() => {
    steps.push("spawn");
    return new Promise<void>((resolve) => {
      finishSpawn = resolve;
    });
  });
  pty.getPtyStatus.mockImplementationOnce(async () => {
    steps.push("status");
    return { foreground: null };
  });
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("fast", "old"));
    });
    markPtyHandoff(["fast"]);
    await act(async () => {
      root.render(sized("fast", "new"));
    });
    // The process is not registered until the spawn lands: asking before then
    // would find nothing and start a second one over it.
    expect(steps).toEqual(["spawn"]);
    await act(async () => {
      finishSpawn();
    });
    await nextFrame();
    expect(steps).toEqual(["spawn", "status"]);
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    expect(pty.killPty).not.toHaveBeenCalled();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("leaves the pane writable and quiet when the repaint nudge fails", async () => {
  const { host, root } = setup();
  const onExit = vi.fn();
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("nudge", "old"));
    });
    markPtyHandoff(["nudge"]);
    pty.getPtyStatus.mockImplementationOnce(alive);
    pty.resizePty
      .mockImplementationOnce(async () => {})
      .mockImplementationOnce(async () => {
        throw new Error("resize failed");
      });
    await act(async () => {
      root.render(sized("nudge", "new", { onExit }));
    });
    await nextFrame();
    expect(onExit).not.toHaveBeenCalled();
    xterm.dataListeners.at(-1)!("x");
    await nextFrame();
    expect(pty.writePty).toHaveBeenCalledWith("nudge", "x");
    // The next fit sends the size again, so the pane does not stay a row short.
    expect(pty.resizePty).toHaveBeenLastCalledWith("nudge", 80, 24);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("attaches rather than spawns when StrictMode replays the effect after a handoff", async () => {
  const { host, root } = setup();
  const strict = (key: string) =>
    createElement(StrictMode, null, sized("replayed", key));
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(strict("old"));
    });
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    markPtyHandoff(["replayed"]);
    pty.getPtyStatus.mockImplementation(alive);
    await act(async () => {
      root.render(strict("new"));
    });
    await nextFrame();
    // The replay of the new view's effect must not read the consumed handoff
    // as "nothing was handed over", and must not kill what it attaches to.
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
    expect(pty.killPty).not.toHaveBeenCalled();
    expect(pty.getPtyStatus).toHaveBeenCalledWith("replayed");
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("finishes tearing down a view that closed before its first fit", async () => {
  // A pane in a hidden tab is never laid out, so an attach waiting for its
  // first fit would wait for ever. Closing must still release it, or the
  // teardown (and the next view for this id) never gets its turn.
  const { host, root } = setup();
  pty.getPtyStatus.mockImplementationOnce(alive);
  try {
    await act(async () => {
      root.render(sized("hidden", "old"));
    });
    markPtyHandoff(["hidden"]);
    await act(async () => {
      root.render(sized("hidden", "new", { active: false }));
    });
    await nextFrame();
    expect(pty.getPtyStatus).toHaveBeenCalledWith("hidden");
    expect(pty.killPty).not.toHaveBeenCalled();
    await act(async () => {
      root.unmount();
    });
    await nextFrame();
    expect(pty.killPty).toHaveBeenCalledExactlyOnceWith("hidden");
  } finally {
    host.remove();
  }
});

it("repaints an attached view only once it has been fitted", async () => {
  const { host, root } = setup();
  pty.getPtyStatus.mockImplementationOnce(alive);
  try {
    await act(async () => {
      root.render(sized("late-fit", "old"));
    });
    markPtyHandoff(["late-fit"]);
    pty.resizePty.mockClear();
    // Inactive and not laid out yet: there is no real size to repaint at.
    await act(async () => {
      root.render(sized("late-fit", "new", { active: false }));
    });
    await nextFrame();
    expect(pty.getPtyStatus).toHaveBeenCalledWith("late-fit");
    expect(pty.resizePty).not.toHaveBeenCalled();
    // The first fit arrives later, when the pane is shown.
    layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
    await act(async () => {
      root.render(sized("late-fit", "new", { active: true }));
    });
    await nextFrame();
    expect(pty.resizePty.mock.calls.slice(0, 2)).toEqual([
      ["late-fit", 80, 23],
      ["late-fit", 80, 24],
    ]);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("sends the size again after a failed repaint nudge", async () => {
  const { host, root } = setup();
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("retry", "old"));
    });
    markPtyHandoff(["retry"]);
    pty.getPtyStatus.mockImplementationOnce(alive);
    pty.resizePty.mockClear();
    pty.resizePty
      .mockImplementationOnce(async () => {})
      .mockImplementationOnce(async () => {
        throw new Error("resize failed");
      });
    await act(async () => {
      root.render(sized("retry", "new"));
    });
    await nextFrame();
    // Step down (ok), step back (fails), the send queued by the first fit, and
    // the one the failure schedules so the pane does not stay a row short.
    const full = pty.resizePty.mock.calls.filter(([, , rows]) => rows === 24);
    expect(full).toHaveLength(3);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

async function exitThenMove(
  id: string,
  makeExit: (
    reportExit: (code: number | null) => void,
  ) => void | Promise<void>,
) {
  const { host, root } = setup();
  const onExit = vi.fn();
  let reportExit!: (code: number | null) => void;
  pty.subscribePty.mockImplementationOnce(((
    _id: string,
    _onData: unknown,
    onExitEvent: (code: number | null) => void,
  ) => {
    reportExit = onExitEvent;
    return () => {};
  }) as never);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized(id, "old"));
    });
    await act(async () => {
      await makeExit(reportExit);
    });
    pty.spawnPty.mockClear();
    markPtyHandoff([id]);
    await act(async () => {
      root.render(sized(id, "new", { onExit }));
    });
    await nextFrame();
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
  return onExit;
}

it("shows the same exit again when a pane that already exited is moved", async () => {
  const onExit = await exitThenMove("exited", (report) => report(3));
  // Not a reattach failure: the process ended before the move and said how.
  expect(onExit).toHaveBeenCalledExactlyOnceWith({ code: 3 });
  expect(pty.getPtyStatus).not.toHaveBeenCalled();
  expect(pty.spawnPty).not.toHaveBeenCalled();
});

it("shows the same spawn error again when a pane that failed to start is moved", async () => {
  pty.spawnPty.mockRejectedValueOnce(new Error("Failed to start claude"));
  const onExit = await exitThenMove("failed", () => {});
  expect(onExit).toHaveBeenCalledExactlyOnceWith({
    code: null,
    error: "Failed to start claude",
  });
  expect(pty.getPtyStatus).not.toHaveBeenCalled();
  expect(pty.spawnPty).not.toHaveBeenCalled();
});

it("still fails visibly when the process died during the move itself", async () => {
  // Alive when the move began, so the view that left reported no exit; the
  // process ended before the next view could attach. That is a lost process,
  // not a known exit, and must be said so rather than spawned over.
  const onExit = await exitThenMove("died-moving", () => {});
  expect(pty.getPtyStatus).toHaveBeenCalledWith("died-moving");
  expect(pty.spawnPty).not.toHaveBeenCalled();
  expect(onExit.mock.calls[0][0].error).toContain("Could not reattach");
});

it("reports the start of a pane that moved while its spawn was in flight", async () => {
  const { host, root } = setup();
  const onSpawn = vi.fn();
  let finishSpawn!: () => void;
  pty.spawnPty.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finishSpawn = resolve;
      }),
  );
  pty.getPtyStatus.mockImplementationOnce(alive);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("mid-spawn", "old", { onSpawn }));
    });
    markPtyHandoff(["mid-spawn"]);
    await act(async () => {
      root.render(sized("mid-spawn", "new", { onSpawn }));
    });
    expect(onSpawn).not.toHaveBeenCalled();
    await act(async () => {
      finishSpawn();
    });
    await nextFrame();
    // The view that attaches launches nothing, so only the one that spawned
    // can say it started - and only once.
    expect(onSpawn).toHaveBeenCalledTimes(1);
    expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  } finally {
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});

it("does not report a start for a spawn whose view was simply closed", async () => {
  const { host, root } = setup();
  const onSpawn = vi.fn();
  let finishSpawn!: () => void;
  pty.spawnPty.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finishSpawn = resolve;
      }),
  );
  try {
    await act(async () => {
      root.render(sized("closed-mid-spawn", "old", { onSpawn }));
    });
    await act(async () => {
      root.unmount();
    });
    await act(async () => {
      finishSpawn();
    });
    expect(onSpawn).not.toHaveBeenCalled();
  } finally {
    host.remove();
  }
});

it("bounds the wait for a handed-off start that never settles, and keeps the bridge until then", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const { host, root } = setup();
  const release = vi.fn();
  pty.holdPtyBridge.mockReturnValueOnce(release);
  pty.spawnPty.mockImplementationOnce(() => new Promise<void>(() => {}));
  pty.getPtyStatus.mockImplementationOnce(alive);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  try {
    await act(async () => {
      root.render(sized("hung", "old"));
    });
    markPtyHandoff(["hung"]);
    await act(async () => {
      root.render(sized("hung", "new"));
    });
    // The departing start is stuck: the next view waits, holding the bridge.
    expect(pty.holdPtyBridge).toHaveBeenCalledTimes(1);
    expect(release).not.toHaveBeenCalled();
    expect(pty.subscribePty).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_900);
    });
    expect(release).not.toHaveBeenCalled();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    // The wait ended on its own: the hold is let go and the view went on.
    expect(release).toHaveBeenCalledTimes(1);
    expect(pty.subscribePty).toHaveBeenCalledTimes(2);
    expect(pty.getPtyStatus).toHaveBeenCalledWith("hung");
  } finally {
    vi.useRealTimers();
    await act(async () => {
      root.unmount();
    });
    host.remove();
  }
});
