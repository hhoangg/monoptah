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
  getPtyStatus: vi.fn(async () => ({ foreground: null })),
}));
vi.mock("../../../platform/tauri/pty", () => pty);
const xterm = vi.hoisted(() => ({
  options: [] as { fontFamily?: string }[],
  titleListeners: [] as ((title: string) => void)[],
}));
vi.mock("../model/terminalLayout", () => ({
  fitTerminal: () => null,
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
    onData() {
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

afterEach(() => {
  xterm.options.length = 0;
  xterm.titleListeners.length = 0;
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
