// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";

const ops = vi.hoisted(() => [] as string[]);
const pty = vi.hoisted(() => ({
  spawnPty: vi.fn(async (..._args: unknown[]) => {}),
  killPty: vi.fn(async () => {}),
  resizePty: vi.fn(async () => {}),
  writePty: vi.fn(async () => {}),
  subscribePty: vi.fn(
    (_id: string, _onData: unknown, _onExit: (code: number | null) => void) =>
      () => {},
  ),
  getPtyStatus: vi.fn(async () => ({ foreground: null })),
}));
vi.mock("../../../platform/tauri/pty", () => pty);
const xterm = vi.hoisted(() => ({
  titleListeners: [] as ((title: string) => void)[],
}));
vi.mock("../../terminal/model/terminalLayout", () => ({
  fitTerminal: () => null,
  applyTerminalChrome: () => {},
}));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
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
    onTitleChange(listener: (title: string) => void) {
      xterm.titleListeners.push(listener);
      return { dispose() {} };
    }
    attachCustomKeyEventHandler() {}
    attachCustomWheelEventHandler() {}
  },
}));
const binary = vi.hoisted(() => ({
  resolveTuiBinary: vi.fn(async (harness: string) => {
    ops.push(`resolve ${harness}`);
    return `/bin/${harness}`;
  }),
}));
vi.mock("../../../integrations/harness/core/tuiBinary", () => binary);
const accounts = vi.hoisted(() => ({
  selected: "default",
  exists: true,
}));
vi.mock("../../providers/model/providerAccounts", async (importOriginal) => ({
  ...(await importOriginal<
    typeof import("../../providers/model/providerAccounts")
  >()),
  selectedProviderAccountId: () => accounts.selected,
  providerAccountExists: () => accounts.exists,
}));

import { newSession, type Session } from "../model/session";
import type { TuiSessionPatch } from "../model/tuiSession";
import { TuiSessionPane } from "./TuiSessionPane";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

let host: HTMLDivElement;
let root: Root;
let now = 1_000_000;

// The pane loads the terminal lazily; load it once up front so a slow first
// import cannot outlast the settle loop.
beforeAll(async () => {
  await import("../../terminal/ui/TerminalView");
});

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(Date, "now").mockImplementation(() => now);
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  host.remove();
  ops.length = 0;
  xterm.titleListeners.length = 0;
  accounts.selected = "default";
  accounts.exists = true;
  now = 1_000_000;
  vi.clearAllMocks();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function tui(
  harness: Parameters<typeof newSession>[0] = "claude",
  extra: Partial<Session> = {},
): Session {
  return {
    ...newSession(harness, "/work/app", undefined, undefined, undefined, {
      surface: "tui",
    }),
    id: `sess-${harness}`,
    ...extra,
  };
}

async function settle() {
  for (let i = 0; i < 6; i++) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }
}

type Handlers = {
  onSessionChange?: (id: string, patch: TuiSessionPatch) => Promise<void>;
  onTitleChange?: (id: string, title: string) => void;
};

function mount(session: Session, handlers: Handlers = {}) {
  const onSessionChange = vi.fn(
    handlers.onSessionChange ??
      (async (_id: string, _patch: TuiSessionPatch) => {
        ops.push("persist");
      }),
  );
  const onTitleChange = vi.fn(handlers.onTitleChange ?? (() => {}));
  const render = (current: Session) =>
    root.render(
      createElement(TuiSessionPane, {
        session: current,
        visible: true,
        focused: true,
        onFocus: () => {},
        onSessionChange,
        onTitleChange,
      }),
    );
  return { render, onSessionChange, onTitleChange };
}

function spawnArgs(call = 0) {
  const [id, cwd, , , launch] = pty.spawnPty.mock.calls[call] as [
    string,
    string,
    number,
    number,
    { program: string; args: string[]; providerAccount?: unknown },
  ];
  return { id, cwd, launch };
}

function reportExit(code: number | null, output?: string) {
  const [, onData, onExit] = pty.subscribePty.mock.calls.at(-1) as [
    string,
    (data: Uint8Array) => void,
    (code: number | null) => void,
  ];
  return act(async () => {
    if (output) onData(new TextEncoder().encode(output));
    onExit(code);
  });
}

const bar = () => host.querySelector("[data-tui-exit]");
const button = (text: string) =>
  [...host.querySelectorAll("button")].find((el) =>
    el.textContent?.includes(text),
  );

it("saves a new Claude conversation id before the CLI starts", async () => {
  pty.spawnPty.mockImplementation(async () => {
    ops.push("spawn");
  });
  const { render, onSessionChange } = mount(tui());
  await act(async () => render(tui()));
  await settle();

  expect(ops).toEqual(["resolve claude", "persist", "spawn"]);
  const patch = onSessionChange.mock.calls[0][1];
  expect(patch.providerSessionId).toMatch(UUID);
  const { launch } = spawnArgs();
  expect(launch.program).toBe("/bin/claude");
  expect(launch.binaryProvider).toBe("claude");
  expect(launch.args).toContain("--session-id");
  expect(launch.args[launch.args.indexOf("--session-id") + 1]).toBe(
    patch.providerSessionId,
  );
  pty.spawnPty.mockImplementation(async () => {});
});

it("runs in the session's PTY id and working copy", async () => {
  const session = tui("claude", { worktreeCwd: "/work/wt" });
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  const { id, cwd } = spawnArgs();
  expect(id).toBe(`tui:${session.id}`);
  expect(cwd).toBe("/work/wt");
});

it("resumes a saved conversation without minting or saving a new id", async () => {
  const session = tui("claude", {
    providerSessionId: "saved-id",
    providerAccountId: "default",
  });
  const { render, onSessionChange } = mount(session);
  await act(async () => render(session));
  await settle();
  const { launch } = spawnArgs();
  expect(launch.args).toContain("--resume");
  expect(launch.args[launch.args.indexOf("--resume") + 1]).toBe("saved-id");
  expect(launch.args).not.toContain("--session-id");
  expect(onSessionChange).not.toHaveBeenCalled();
});

it("starts with the selected non-default account and pins it to the session", async () => {
  accounts.selected = "work";
  const { render, onSessionChange } = mount(tui());
  await act(async () => render(tui()));
  await settle();
  expect(spawnArgs().launch.providerAccount).toEqual({
    provider: "claude",
    id: "work",
  });
  expect(onSessionChange.mock.calls[0][1].providerAccountId).toBe("work");
});

it("keeps a pinned account when the selection has moved on", async () => {
  accounts.selected = "other";
  const session = tui("claude", {
    providerSessionId: "saved-id",
    providerAccountId: "work",
  });
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(spawnArgs().launch.providerAccount).toEqual({
    provider: "claude",
    id: "work",
  });
});

it("sends no account env for the default account", async () => {
  const { render } = mount(tui());
  await act(async () => render(tui()));
  await settle();
  expect(spawnArgs().launch.providerAccount).toBeUndefined();
});

it("refuses to start under an account that was removed", async () => {
  accounts.exists = false;
  const session = tui("claude", { providerAccountId: "gone" });
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(pty.spawnPty).not.toHaveBeenCalled();
  expect(bar()?.textContent).toContain("removed provider account");
});

it("launches other providers fresh with their bare CLI", async () => {
  const session = tui("codex");
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  const { launch } = spawnArgs();
  expect(launch.program).toBe("/bin/codex");
  expect(launch.args).toEqual([]);
});

it("saves nothing for a provider with no conversation id and no accounts", async () => {
  const session = tui("cursor");
  const { render, onSessionChange } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(onSessionChange).not.toHaveBeenCalled();
  expect(spawnArgs().launch).toEqual({
    program: "/bin/cursor",
    args: [],
    binaryProvider: "cursor",
  });
});

it("does not start the CLI when the conversation id cannot be saved", async () => {
  const session = tui();
  const { render, onSessionChange } = mount(session, {
    onSessionChange: async () => {
      throw new Error("disk full");
    },
  });
  await act(async () => render(session));
  await settle();
  expect(onSessionChange).toHaveBeenCalledTimes(1);
  expect(pty.spawnPty).not.toHaveBeenCalled();
  expect(bar()?.textContent).toContain("Could not start Claude Code");
  expect(bar()?.textContent).toContain("disk full");
  expect(button("Restart")).toBeDefined();
});

it("shows a readable bar when the CLI exits right away", async () => {
  const session = tui("codex");
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(bar()).toBeNull();
  now += 200;
  await reportExit(1);
  expect(bar()?.textContent).toContain("Process exited (code 1)");
  expect(bar()?.textContent).toContain("interactive mode");
  expect(button("Restart")).toBeDefined();
});

it("shows only the code for an exit well after launch", async () => {
  const session = tui("codex");
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(0);
  expect(bar()?.textContent).toContain("Process exited (code 0)");
  expect(bar()?.textContent).not.toContain("interactive mode");
});

it("shows why a process could not be started at all", async () => {
  pty.spawnPty.mockRejectedValueOnce(new Error("Failed to start codex: nope"));
  const session = tui("codex");
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(bar()?.textContent).toContain("Could not start Codex");
  expect(bar()?.textContent).toContain("Failed to start codex: nope");
});

it("restarts Claude into the same conversation", async () => {
  const first = tui();
  const { render, onSessionChange } = mount(first);
  await act(async () => render(first));
  await settle();
  const savedId = onSessionChange.mock.calls[0][1].providerSessionId as string;

  // The app applies the patch, so the pane now sees the saved id.
  const saved = {
    ...first,
    providerSessionId: savedId,
    providerAccountId: "default",
  };
  await act(async () => render(saved));
  now += 60_000;
  await reportExit(0);
  await act(async () => {
    button("Restart")?.click();
  });
  await settle();

  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  const { launch } = spawnArgs(1);
  expect(launch.args).toContain("--resume");
  expect(launch.args[launch.args.indexOf("--resume") + 1]).toBe(savedId);
  expect(launch.args).not.toContain("--session-id");
  expect(onSessionChange).toHaveBeenCalledTimes(1);
  expect(bar()).toBeNull();
});

it("offers a fresh conversation when resuming fails at once", async () => {
  const session = tui("claude", {
    providerSessionId: "never-saved",
    providerAccountId: "default",
  });
  // The app applies a patch to the session before the save settles.
  const { render, onSessionChange } = mount(session, {
    onSessionChange: async (_id, patch) => {
      if (patch.providerSessionId === null) {
        render({ ...session, providerSessionId: undefined });
      }
    },
  });
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(1);
  expect(button("Start new conversation")).toBeDefined();

  await act(async () => {
    button("Start new conversation")?.click();
  });
  expect(onSessionChange).toHaveBeenCalledWith(session.id, {
    providerSessionId: null,
  });
  await settle();
  const { launch } = spawnArgs(1);
  expect(launch.args).toContain("--session-id");
  expect(launch.args).not.toContain("--resume");
});

it("does not offer a fresh start after a normal late exit", async () => {
  const session = tui("claude", {
    providerSessionId: "saved",
    providerAccountId: "default",
  });
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(0);
  expect(button("Start new conversation")).toBeUndefined();
});

it("titles a placeholder tab from the terminal title", async () => {
  const session = tui();
  const { render, onTitleChange } = mount(session);
  await act(async () => render(session));
  await settle();
  xterm.titleListeners[0]("✳ Fix login bug");
  expect(onTitleChange).toHaveBeenCalledExactlyOnceWith(
    session.id,
    "Fix login bug",
  );
});

it("leaves a title the app or user chose alone", async () => {
  const session = tui("claude", { title: "#12 Fix the bug" });
  const { render, onTitleChange } = mount(session);
  await act(async () => render(session));
  await settle();
  xterm.titleListeners[0]("Claude Code");
  expect(onTitleChange).not.toHaveBeenCalled();
});

it("shows the new-worktree option as visibly unavailable", async () => {
  const session = tui();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  const control = button("New worktree unavailable") as HTMLButtonElement;
  expect(control.disabled).toBe(true);
  expect(control.title).toContain("terminal sessions");
});

const NO_CONVERSATION =
  "No\u001b[4Gconversation\u001b[17Gfound\u001b[23Gwith\u001b[28Gsession\u001b[36GID:\u001b[40Gsaved-id\r\r\n";

function resuming() {
  return tui("claude", {
    providerSessionId: "saved-id",
    providerAccountId: "default",
  });
}

it("claims the saved id once when --resume finds no conversation", async () => {
  const session = resuming();
  const { render, onSessionChange } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(spawnArgs(0).launch.args).toContain("--resume");

  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();

  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  const { launch } = spawnArgs(1);
  expect(launch.args).toContain("--session-id");
  expect(launch.args[launch.args.indexOf("--session-id") + 1]).toBe("saved-id");
  expect(launch.args).not.toContain("--resume");
  // Same id, so there is nothing new to save, and no error was shown.
  expect(onSessionChange).not.toHaveBeenCalled();
  expect(bar()).toBeNull();
});

it("shows the error when the claim also fails, and offers a fresh start", async () => {
  const session = resuming();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  now += 100;
  await reportExit(1, "Error: Session ID saved-id is already in use.\r\n");
  await settle();

  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  expect(bar()?.textContent).toContain("Process exited (code 1)");
  expect(button("Start new conversation")).toBeDefined();
});

it("does not claim a second time when the claim again finds nothing", async () => {
  const session = resuming();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  expect(bar()).not.toBeNull();
});

it("lets a manual restart claim the id again", async () => {
  const session = resuming();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  now += 100;
  await reportExit(1, "Error: Session ID saved-id is already in use.\r\n");
  await act(async () => {
    button("Restart")?.click();
  });
  await settle();
  expect(spawnArgs(2).launch.args).toContain("--resume");
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(4);
  expect(spawnArgs(3).launch.args).toContain("--session-id");
});

it("does not claim for any other early exit", async () => {
  const session = resuming();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(1, "Error: something else went wrong\r\n");
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(bar()?.textContent).toContain("Process exited (code 1)");
  expect(button("Start new conversation")).toBeDefined();
});

it("does not claim when the message appears long after launch", async () => {
  const session = resuming();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(bar()).not.toBeNull();
});

it("does not claim a first launch that was not resuming", async () => {
  const session = tui();
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(bar()).not.toBeNull();
});
