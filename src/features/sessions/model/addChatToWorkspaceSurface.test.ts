// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import { saveProviderSurface } from "../../providers/model/providerSurface";
import { HARNESSES } from "./session";
import {
  newFileTab,
  newTab,
  type WorkspaceTab,
} from "../../workspace/model/layout";
import { applyAddToChatRequest } from "./addChatToWorkspace";

beforeEach(() => {
  localStorage.clear();
  // Every provider prefers a terminal, so any chat below is a deliberate pin.
  for (const harness of HARNESSES) saveProviderSurface(harness, "tui");
});

function newChat(
  result: NonNullable<ReturnType<typeof applyAddToChatRequest>>,
) {
  return result.sessions.find((entry) => entry.id === result.sessionId)!;
}

describe("add to chat surface", () => {
  it("opens the zero-tab fallback chat in chat even when the preference is a terminal", () => {
    const result = applyAddToChatRequest({
      sessions: [],
      tabs: [],
      projectCwd: "/current/project",
      text: "selected code",
    });
    expect(newChat(result!).surface).toBe("chat");
    expect(newChat(result!).composerSeed).toContain("selected code");
  });

  it("opens the chat beside a file pane in chat even when the preference is a terminal", () => {
    const file = newFileTab("/current/project/readme.md", "/current/project");
    const tab: WorkspaceTab = {
      ...newTab("unused"),
      id: "tab1",
      editorPanes: [{ id: "pane", files: [file], activeFileId: file.id }],
    };
    const result = applyAddToChatRequest({
      sessions: [],
      tabs: [tab],
      activeTabId: "tab1",
      projectCwd: "/current/project",
      text: "selected code",
    });
    expect(newChat(result!).surface).toBe("chat");
    expect(newChat(result!).composerSeed).toContain("selected code");
  });
});
