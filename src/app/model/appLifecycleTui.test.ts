import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  bindHarnessSession,
  forgetHarnessSession,
  isLiveHarness,
} from "../../integrations/harness/core/registry";
import { newSession } from "../../features/sessions/model/session";
import { bindResumedSessions, reapWindowRuntime } from "./appLifecycle";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  ask: vi.fn().mockResolvedValue(true),
}));
vi.mock("./windowTransferBootstrap", () => ({
  loadWindowTransfer: vi.fn().mockResolvedValue(null),
}));
vi.mock("../../integrations/harness/core/registry", () => ({
  bindHarnessSession: vi.fn(),
  isLiveHarness: vi.fn(),
  forgetHarnessSession: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("../../integrations/harness/core/child", () => ({
  killAllChildren: vi.fn().mockResolvedValue(undefined),
}));

const terminal = {
  ...newSession("claude", "/alpha", undefined, undefined, undefined, {
    surface: "tui",
  }),
  id: "term",
  providerSessionId: "claude-conversation",
};
const chat = {
  ...newSession("claude", "/alpha"),
  id: "chat",
  providerSessionId: "claude-thread",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(isLiveHarness).mockReturnValue(true);
});

describe("window reap", () => {
  it("kills a terminal session's PTY, which no host exit hook reaches for a closed window", async () => {
    await reapWindowRuntime([terminal, chat], [], [], false);
    const killed = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === "pty_kill")
      .map(([, args]) => args);
    expect(killed).toEqual([{ id: "tui:term" }]);
  });

  it("only forgets chat sessions in the adapter", async () => {
    await reapWindowRuntime([terminal, chat], [], [], false);
    expect(forgetHarnessSession).toHaveBeenCalledTimes(1);
    expect(forgetHarnessSession).toHaveBeenCalledWith("claude", "chat");
  });
});

describe("resume binding", () => {
  it("never hands a terminal session's conversation id to a chat adapter", () => {
    bindResumedSessions([terminal, chat]);
    expect(bindHarnessSession).toHaveBeenCalledTimes(1);
    expect(bindHarnessSession).toHaveBeenCalledWith(
      "claude",
      "chat",
      "claude-thread",
      "/alpha",
      undefined,
      chat.blocks,
    );
  });
});
