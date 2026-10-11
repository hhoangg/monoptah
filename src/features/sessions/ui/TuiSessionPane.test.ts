// @vitest-environment happy-dom
import { act, createElement, StrictMode } from "react";
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
  titleListeners: [] as ((title: string) => void)[],
}));
const layout = vi.hoisted(() => ({
  fitTerminal: vi.fn((): { cols: number; rows: number } | null => null),
}));
vi.mock("../../terminal/model/terminalLayout", () => ({
  fitTerminal: layout.fitTerminal,
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
import type { TuiLaunchEvent, TuiSessionPatch } from "../model/tuiSession";
import {
  consumePtyHandoff,
  markPtyHandoff,
  settlePtyHandoffs,
} from "../../terminal/model/ptyHandoff";
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
  settlePtyHandoffs();
  layout.fitTerminal.mockReset();
  pty.getPtyStatus.mockReset();
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
  onLaunchEvent?: (id: string, event: TuiLaunchEvent) => void;
};

function mount(session: Session, handlers: Handlers = {}) {
  const onSessionChange = vi.fn(
    handlers.onSessionChange ??
      (async (_id: string, _patch: TuiSessionPatch) => {
        ops.push("persist");
      }),
  );
  const onTitleChange = vi.fn(handlers.onTitleChange ?? (() => {}));
  const onLaunchEvent = vi.fn(handlers.onLaunchEvent ?? (() => {}));
  const onExitClose = vi.fn();
  const render = (current: Session) =>
    root.render(
      createElement(TuiSessionPane, {
        session: current,
        visible: true,
        focused: true,
        onFocus: () => {},
        onSessionChange,
        onTitleChange,
        onLaunchEvent,
        onExitClose,
      }),
    );
  return { render, onSessionChange, onTitleChange, onLaunchEvent, onExitClose };
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

it("shows only the code for a late exit with a failure code", async () => {
  const session = tui("codex");
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(2);
  expect(bar()?.textContent).toContain("Process exited (code 2)");
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
  await reportExit(2);
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

it("does not offer a fresh start after a late exit with a failure code", async () => {
  const session = tui("claude", {
    providerSessionId: "saved",
    providerAccountId: "default",
  });
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(2);
  expect(button("Start new conversation")).toBeUndefined();
});

it("closes the pane after a clean exit well after launch", async () => {
  const session = tui("claude");
  const { render, onLaunchEvent, onExitClose } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(0);
  expect(onExitClose).toHaveBeenCalledTimes(1);
  expect(onExitClose).toHaveBeenCalledWith(session.id);
  // The exit is still reported, and before the close.
  const kinds = onLaunchEvent.mock.calls.map(([, event]) => event.kind);
  expect(kinds).toEqual(["started", "exit"]);
  expect(onLaunchEvent.mock.invocationCallOrder.at(-1)).toBeLessThan(
    onExitClose.mock.invocationCallOrder[0],
  );
});

it("keeps the pane open for a clean exit right after launch", async () => {
  const session = tui("codex");
  const { render, onExitClose } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 200;
  await reportExit(0);
  expect(onExitClose).not.toHaveBeenCalled();
  expect(bar()?.textContent).toContain("Process exited (code 0)");
});

it("keeps the pane open for a late exit with a failure code", async () => {
  const session = tui("codex");
  const { render, onExitClose } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(1);
  expect(onExitClose).not.toHaveBeenCalled();
  expect(bar()?.textContent).toContain("Process exited (code 1)");
});

it("keeps the pane open when the process could not start", async () => {
  pty.spawnPty.mockRejectedValueOnce(new Error("Failed to start codex: nope"));
  const session = tui("codex");
  const { render, onExitClose } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(onExitClose).not.toHaveBeenCalled();
  expect(bar()?.textContent).toContain("Failed to start codex: nope");
});

it("does not close the pane while retrying a --resume that found nothing", async () => {
  const session = tui("claude", {
    providerSessionId: "saved-id",
    providerAccountId: "default",
  });
  const { render, onExitClose } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 100;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  expect(onExitClose).not.toHaveBeenCalled();
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

it("hides the new-worktree option for a session that already has a worktree", async () => {
  const session = tui("claude", { worktreeCwd: "/work/wt" });
  const { render } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(button("New worktree unavailable")).toBeUndefined();
});

it("reports started once, after the PTY has spawned", async () => {
  const session = tui();
  const { render, onLaunchEvent } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(onLaunchEvent).toHaveBeenCalledExactlyOnceWith(session.id, {
    kind: "started",
  });
});

it("reports no started event when the process fails to spawn", async () => {
  pty.spawnPty.mockRejectedValueOnce(new Error("nope"));
  const session = tui("codex");
  const { render, onLaunchEvent } = mount(session);
  await act(async () => render(session));
  await settle();
  expect(onLaunchEvent.mock.calls.map(([, event]) => event.kind)).toEqual([
    "exit",
  ]);
});

it("reports an early exit", async () => {
  const session = tui("codex");
  const { render, onLaunchEvent } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 200;
  await reportExit(1);
  expect(onLaunchEvent).toHaveBeenLastCalledWith(session.id, {
    kind: "exit",
    exit: { code: 1, error: undefined, early: true },
  });
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
  const { render, onSessionChange, onLaunchEvent } = mount(session);
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
  // The retry is not an exit: it reports a fresh start and nothing else.
  expect(onLaunchEvent.mock.calls.map(([, event]) => event.kind)).toEqual([
    "started",
    "started",
  ]);
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

function mountSplit(inSplit: boolean) {
  const onPaneDragStart = vi.fn();
  const onClose = vi.fn();
  const session = tui();
  const render = () =>
    root.render(
      createElement(TuiSessionPane, {
        session,
        visible: true,
        focused: true,
        onFocus: () => {},
        onSessionChange: async () => {},
        onTitleChange: () => {},
        inSplit,
        onPaneDragStart,
        onClose,
      }),
    );
  return { render, onPaneDragStart, onClose, session };
}

const closeButton = () =>
  host.querySelector<HTMLButtonElement>('button[aria-label="Close pane"]');
const header = () =>
  host.querySelector<HTMLElement>("[data-tui-pane] > div:first-child")!;
const press = (target: Element) =>
  act(() => {
    target.dispatchEvent(
      new PointerEvent("pointerdown", { bubbles: true, button: 0 }),
    );
  });

it("shows the drag handle and close button only in a split", async () => {
  const alone = mountSplit(false);
  await act(async () => alone.render());
  await settle();
  expect(closeButton()).toBeNull();
  expect(header().className).not.toContain("cursor-grab");
  press(header());
  expect(alone.onPaneDragStart).not.toHaveBeenCalled();

  const split = mountSplit(true);
  await act(async () => split.render());
  await settle();
  expect(closeButton()).not.toBeNull();
  expect(header().className).toContain("cursor-grab");
  press(header());
  expect(split.onPaneDragStart).toHaveBeenCalledTimes(1);
  await act(async () => closeButton()!.click());
  expect(split.onClose).toHaveBeenCalledExactlyOnceWith(split.session.id);
});

it("does not start a pane drag from a data-no-drag control", async () => {
  const split = mountSplit(true);
  await act(async () => split.render());
  await settle();
  press(closeButton()!);
  press(button("New worktree unavailable")!);
  expect(split.onPaneDragStart).not.toHaveBeenCalled();
});

it("keeps the pane and offers a fresh start when --resume finds no conversation late", async () => {
  const session = resuming();
  const { render, onExitClose } = mount(session);
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(0, NO_CONVERSATION);
  await settle();

  expect(onExitClose).not.toHaveBeenCalled();
  // A slow start does not retry: only the prompt exit claims the saved id.
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(bar()?.textContent).toContain("could not find the saved conversation");
  expect(button("Start new conversation")).toBeDefined();
  expect(button("Restart")).toBeDefined();
});

const alive = async () => ({ foreground: null });

/** Lets the first animation frame run, where the terminal is first fitted. */
async function nextFrame() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 40));
  });
}

/**
 * Launches the pane, then moves it the way a detach does: the departure is
 * marked, the old view unmounts, and the caller renders the new one.
 */
async function launchThenMove(session: Session) {
  const handlers = mount(session);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  await act(async () => handlers.render(session));
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  markPtyHandoff([`tui:${session.id}`]);
  await act(async () => root.render(null));
  expect(pty.killPty).not.toHaveBeenCalled();
  return handlers;
}

it("attaches after a move without relaunching and keeps the real launch time", async () => {
  const session = tui("claude", { id: "moved-session" });
  const { render, onSessionChange, onLaunchEvent, onExitClose } =
    await launchThenMove(session);
  expect(ops).toEqual(["resolve claude", "persist"]);
  expect(onLaunchEvent).toHaveBeenCalledWith(session.id, { kind: "started" });

  // The pane moves to another tab long after the launch: same session, new view.
  now += 60_000;
  pty.getPtyStatus.mockImplementationOnce(alive);
  await act(async () => render(session));
  await settle();
  await nextFrame();

  // Nothing is launched or sent: no new binary lookup, saved id or spawn.
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(ops).toEqual(["resolve claude", "persist"]);
  expect(onSessionChange).toHaveBeenCalledTimes(1);
  expect(onLaunchEvent).toHaveBeenCalledTimes(1);
  expect(pty.resizePty).toHaveBeenCalled();

  // A clean exit a second after the attach is not an early exit.
  now += 1_000;
  await reportExit(0);
  await settle();
  expect(onExitClose).toHaveBeenCalledExactlyOnceWith(session.id);
});

it("still counts a quick exit after a move as early when the launch was recent", async () => {
  const session = tui("claude", { id: "quick-session" });
  const { render, onExitClose } = await launchThenMove(session);
  now += 1_000;
  pty.getPtyStatus.mockImplementationOnce(alive);
  await act(async () => render(session));
  await settle();
  await nextFrame();

  now += 1_000;
  await reportExit(0);
  await settle();
  expect(onExitClose).not.toHaveBeenCalled();
});

it("does not attach to a live process the pane was not handed (regression guard)", async () => {
  // A second window, a reload or a restore finds the same ids; the process is
  // alive but belongs to another view, so this pane launches as it always did.
  pty.getPtyStatus.mockImplementation(alive);
  const session = tui("claude", { id: "foreign-session" });
  const { render } = mount(session);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  await act(async () => render(session));
  await settle();
  await nextFrame();

  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(ops).toEqual(["resolve claude", "persist"]);
  expect(pty.resizePty).not.toHaveBeenCalledWith(`tui:${session.id}`, 80, 23);
});

it("does not launch anything when the process is gone by the time the pane attaches", async () => {
  const session = tui("claude", { id: "raced-session" });
  const { render, onLaunchEvent } = await launchThenMove(session);
  // The default status check rejects: the process died in between.
  await act(async () => render(session));
  await settle();
  await nextFrame();

  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(ops).toEqual(["resolve claude", "persist"]);
  expect(bar()?.textContent).toContain("Could not reattach");
  expect(button("Restart")).toBeDefined();
  expect(onLaunchEvent).toHaveBeenLastCalledWith(
    session.id,
    expect.objectContaining({ kind: "exit" }),
  );
});

it("launches again on Restart after a failed attach", async () => {
  const session = tui("claude", { id: "restart-session" });
  const { render } = await launchThenMove(session);
  await act(async () => render(session));
  await settle();
  await nextFrame();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);

  await act(async () => {
    button("Restart")?.click();
  });
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
});

it("keeps offering a fresh conversation after the pane was moved", async () => {
  const session = resuming();
  const { render } = await launchThenMove(session);
  pty.getPtyStatus.mockImplementationOnce(alive);
  await act(async () => render(session));
  await settle();
  await nextFrame();

  now += 60_000;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(button("Start new conversation")).toBeDefined();
});

it("does not offer a fresh conversation after a move when the launch minted a new id", async () => {
  const session = tui("claude", { id: "minted-session" });
  const { render } = await launchThenMove(session);
  pty.getPtyStatus.mockImplementationOnce(alive);
  await act(async () => render(session));
  await settle();
  await nextFrame();

  now += 60_000;
  await reportExit(0, NO_CONVERSATION);
  await settle();
  expect(button("Start new conversation")).toBeUndefined();
});

it("keeps the pane when an attach has no launch record and the process exits at once", async () => {
  const session = tui("claude", { id: "no-record-session" });
  markPtyHandoff([`tui:${session.id}`]);
  consumePtyHandoff(`tui:${session.id}`);
  const { render, onExitClose } = mount(session);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  pty.getPtyStatus.mockImplementationOnce(alive);
  await act(async () => render(session));
  await settle();
  await nextFrame();

  expect(pty.spawnPty).not.toHaveBeenCalled();
  now += 1_000;
  await reportExit(0);
  await settle();
  expect(onExitClose).not.toHaveBeenCalled();
  expect(bar()).not.toBeNull();
});

it("attaches rather than launches when StrictMode replays the effect after a move", async () => {
  const session = tui("claude", { id: "strict-session" });
  await launchThenMove(session);
  pty.getPtyStatus.mockImplementation(alive);
  const strictRender = () =>
    root.render(
      createElement(
        StrictMode,
        null,
        createElement(TuiSessionPane, {
          session,
          visible: true,
          focused: true,
          onFocus: () => {},
          onSessionChange: async () => {},
          onTitleChange: () => {},
        }),
      ),
    );
  await act(async () => strictRender());
  await settle();
  await nextFrame();

  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(pty.killPty).not.toHaveBeenCalled();
  expect(pty.getPtyStatus).toHaveBeenCalledWith(`tui:${session.id}`);
});

it("keeps the real exit notice and its buttons when an exited pane is moved", async () => {
  const session = tui("claude", {
    id: "exited-moved",
    providerSessionId: "saved-id",
    providerAccountId: "default",
  });
  const { render, onLaunchEvent } = mount(session);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(1, NO_CONVERSATION);
  await settle();
  const before = bar()?.textContent;
  expect(before).toContain("Process exited (code 1)");
  expect(button("Start new conversation")).toBeDefined();
  onLaunchEvent.mockClear();

  // Moved to another tab after it ended: the process is gone, nothing to attach.
  markPtyHandoff([`tui:${session.id}`]);
  await act(async () => root.render(null));
  await act(async () => render(session));
  await settle();
  await nextFrame();

  expect(pty.getPtyStatus).not.toHaveBeenCalled();
  expect(pty.spawnPty).toHaveBeenCalledTimes(1);
  expect(bar()?.textContent).toBe(before);
  expect(bar()?.textContent).not.toContain("Could not reattach");
  expect(button("Start new conversation")).toBeDefined();
  expect(button("Restart")).toBeDefined();
  // The exit was already reported once; moving does not report it again.
  expect(onLaunchEvent).not.toHaveBeenCalled();

  // Restart still launches, since the process really is gone.
  await act(async () => {
    button("Restart")?.click();
  });
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
});

it("does not carry an old exit into a pane that was launched again", async () => {
  const session = tui("claude", { id: "relaunched-moved" });
  const { render } = mount(session);
  layout.fitTerminal.mockReturnValue({ cols: 80, rows: 24 });
  await act(async () => render(session));
  await settle();
  now += 60_000;
  await reportExit(1);
  await settle();
  await act(async () => {
    button("Restart")?.click();
  });
  await settle();
  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  expect(bar()).toBeNull();

  // It is alive again, so a move attaches; the earlier exit is history.
  markPtyHandoff([`tui:${session.id}`]);
  await act(async () => root.render(null));
  pty.getPtyStatus.mockImplementationOnce(alive);
  await act(async () => render(session));
  await settle();
  await nextFrame();
  expect(pty.getPtyStatus).toHaveBeenCalledWith(`tui:${session.id}`);
  expect(pty.spawnPty).toHaveBeenCalledTimes(2);
  expect(bar()).toBeNull();
});
