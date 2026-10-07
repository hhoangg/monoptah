import { afterEach, describe, expect, it, vi } from "vitest";
import { newTab } from "../../workspace/model/layout";
import { newSession, type Session } from "./session";
import { createSessionRemover } from "./sessionRemoval";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  forget: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../../integrations/harness/core/registry", () => ({
  forgetHarnessSession: mocks.forget,
}));

function run(mode: "archive" | "delete", surface: "tui" | "chat") {
  const closing: Session = newSession(
    "claude",
    "/tmp/project",
    undefined,
    undefined,
    undefined,
    { surface },
  );
  const other = newSession("claude", "/tmp/project");
  const state = {
    tabs: [newTab(closing.id), newTab(other.id)],
    sessions: [closing, other],
    activeTabId: "",
    dirtyFiles: new Set<string>(),
  };
  state.activeTabId = state.tabs[0].id;
  mocks.invoke.mockImplementation(async (command: string, args: any) =>
    command === "session_upsert" ? { ...args.session } : undefined,
  );
  const remover = createSessionRemover({
    mode,
    replacement: { cwd: "/tmp/project", harness: "claude" },
    workspace: { snapshot: () => state, apply: () => {} },
    confirm: async () => true,
    stop: async () => {},
  });
  return { closing, done: remover.remove(closing.id) };
}

const killed = () =>
  mocks.invoke.mock.calls
    .filter(([command]) => command === "pty_kill")
    .map(([, args]) => (args as { id: string }).id);

afterEach(() => {
  mocks.invoke.mockReset();
  mocks.forget.mockClear();
});

describe.each(["archive", "delete"] as const)(
  "closing a terminal session (%s)",
  (mode) => {
    it("kills its PTY before the session leaves the workspace", async () => {
      const { closing, done } = run(mode, "tui");
      expect(await done).toBe(true);
      expect(killed()).toEqual([`tui:${closing.id}`]);
    });

    it("keeps the conversation handle: archive saves the row before archiving it", async () => {
      const { closing, done } = run(mode, "tui");
      await done;
      const commands = mocks.invoke.mock.calls.map(([command]) => command);
      if (mode === "archive") {
        const upsert = mocks.invoke.mock.calls.find(
          ([command]) => command === "session_upsert",
        );
        expect(upsert?.[1].session).toMatchObject({
          id: closing.id,
          surface: "tui",
        });
        expect(commands.indexOf("session_upsert")).toBeLessThan(
          commands.indexOf("session_set_archived"),
        );
      } else {
        expect(commands).toContain("session_delete");
      }
    });

    it("does not touch the chat adapter", async () => {
      await run(mode, "tui").done;
      expect(mocks.forget).not.toHaveBeenCalled();
    });

    it("leaves a chat session's cleanup as it was", async () => {
      const { closing, done } = run(mode, "chat");
      await done;
      // Archive forgets after the commit without awaiting; let it settle.
      await Promise.resolve();
      expect(killed()).toEqual([]);
      expect(mocks.forget).toHaveBeenCalledWith("claude", closing.id);
    });
  },
);
