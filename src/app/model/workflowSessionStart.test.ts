// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Mono } from "../../features/monos/model/mono";
import { createMonoLaunchRegistry } from "../../features/monos/model/monoLaunches";
import {
  resetHarnessModelOverlays,
  setHarnessModels,
} from "../../features/sessions/model/models";
import { loadSessionFolders } from "../../features/sessions/model/sessionFolders";
import { newSession } from "../../features/sessions/model/session";
import {
  startWorkflowSession,
  type WorkflowSessionDeps,
} from "./workflowSessionStart";

vi.mock("../../integrations/harness/core/availability", () => ({
  isHarnessAvailable: (id: string) => id === "codex" || id === "grok",
}));

const values = new Map<string, string>();
vi.stubGlobal("localStorage", {
  clear: () => values.clear(),
  getItem: (key: string) => values.get(key) ?? null,
  setItem: (key: string, value: string) => values.set(key, value),
  removeItem: (key: string) => values.delete(key),
});

beforeEach(() => {
  localStorage.clear();
  setHarnessModels("codex", [
    { id: "codex:test", harness: "codex", name: "Test model" },
  ]);
});
afterEach(() => resetHarnessModelOverlays());

const mono: Mono = {
  id: "mono-1",
  sessionId: "chat",
  mascot: "skull",
  color: "#5b8def",
  projects: ["/code/app"],
};

function fixture() {
  const chat = {
    ...newSession("codex", "/home/user", "codex:test"),
    id: "chat",
  };
  const registry = createMonoLaunchRegistry();
  const open = [chat];
  const launched: Array<{ launch: unknown; id: string }> = [];
  const deps: WorkflowSessionDeps = {
    findMono: (id) => (id === mono.id ? mono : undefined),
    openSession: async (id) => (id === chat.id ? chat : null),
    launch: vi.fn(async (launch, id, onSettled) => {
      launched.push({ launch, id });
      open.push({ ...newSession("codex", "/code/app", "codex:test"), id });
      onSettled({ status: "completed", text: "done" });
    }),
    registry,
    isMonoSession: (id) => id === chat.id,
    openSessions: () => open,
    storedSessions: async () => [],
    storedSession: async () => null,
    worktrees: async () => ({ worktrees: [], defaultRoot: "/wt" }),
  };
  return { deps, registry, launched };
}

describe("startWorkflowSession", () => {
  it("starts a full-access session without a completion report and files it in a folder", async () => {
    const { deps, registry, launched } = fixture();
    const started = await startWorkflowSession(deps, "mono-1", {
      project: "/code/app",
      prompt: "Do it",
      workspaceMode: "current",
      folderName: "Ship it · 2026-10-22 10:00",
    });
    expect(started.sessionId).toMatch(/^app-chat-wf[0-9a-f]{32}$/);
    expect(launched[0].launch).toMatchObject({
      cwd: "/code/app",
      prompt: "Do it",
      runtimeMode: "full-access",
      workspaceMode: "current",
    });
    // Always visible, so the run can be followed from the sidebar.
    expect(launched[0].launch).not.toHaveProperty("sidebarHidden");
    expect(registry.get(started.sessionId)).toEqual({
      monoId: "mono-1",
      parented: false,
    });
    expect(await started.settled).toMatchObject({ status: "completed" });
    const folders = loadSessionFolders("/code/app");
    expect(folders).toHaveLength(1);
    expect(folders[0].name).toBe("Ship it · 2026-10-22 10:00");
    expect(folders[0].sessionIds).toEqual([started.sessionId]);
    expect(started.folderId).toBe(folders[0].id);

    // The next session of the run reuses that folder.
    const second = await startWorkflowSession(deps, "mono-1", {
      project: "/code/app",
      prompt: "Review it",
      workspaceMode: "current",
      folderId: started.folderId,
      folderName: "Ship it · 2026-10-22 10:00",
    });
    expect(loadSessionFolders("/code/app")).toHaveLength(1);
    expect(loadSessionFolders("/code/app")[0].sessionIds).toEqual([
      started.sessionId,
      second.sessionId,
    ]);
  });

  it("nests under a session this Mono started, and drops a parent it does not know", async () => {
    const { deps, registry } = fixture();
    const parent = await startWorkflowSession(deps, "mono-1", {
      project: "/code/app",
      prompt: "Dev",
      workspaceMode: "current",
    });
    const child = await startWorkflowSession(deps, "mono-1", {
      project: "/code/app",
      prompt: "Review",
      workspaceMode: "current",
      parentSessionId: parent.sessionId,
    });
    expect(child.sessionId.startsWith(`app-${parent.sessionId}-`)).toBe(true);
    expect(registry.get(child.sessionId)?.parented).toBe(true);

    // A child cannot be a parent, and a session the app forgot is no parent.
    for (const parentSessionId of [child.sessionId, "app-gone-dev"]) {
      const flat = await startWorkflowSession(deps, "mono-1", {
        project: "/code/app",
        prompt: "Review",
        workspaceMode: "current",
        parentSessionId,
      });
      expect(flat.sessionId.startsWith("app-chat-")).toBe(true);
    }
  });

  it("starts a model in its own harness when the node sets only a model", async () => {
    const { deps, launched } = fixture();
    // The Mono runs on codex; grok:grok-4.6 belongs to grok.
    await startWorkflowSession(deps, "mono-1", {
      project: "/code/app",
      prompt: "Do it",
      workspaceMode: "current",
      model: "grok:grok-4.6",
    });
    expect(launched[0].launch).toMatchObject({
      harness: "grok",
      model: "grok:grok-4.6",
    });
  });

  it("names the model when the catalog no longer has it", async () => {
    const { deps, launched } = fixture();
    await expect(
      startWorkflowSession(deps, "mono-1", {
        project: "/code/app",
        prompt: "Do it",
        workspaceMode: "current",
        harness: "codex",
        model: "codex:gone",
      }),
    ).rejects.toThrow(
      'Model "codex:gone" is not available. Pick one in the step\'s model list.',
    );
    expect(launched).toEqual([]);
  });

  it("names the effort when the model does not offer it", async () => {
    const { deps, launched } = fixture();
    await expect(
      startWorkflowSession(deps, "mono-1", {
        project: "/code/app",
        prompt: "Do it",
        workspaceMode: "current",
        harness: "grok",
        model: "grok:grok-4.6",
        effort: "turbo",
      }),
    ).rejects.toThrow(
      'Effort "turbo" is not available for this model. Pick one in the step\'s model list.',
    );
    // A model with no effort setting at all reads the same.
    await expect(
      startWorkflowSession(deps, "mono-1", {
        project: "/code/app",
        prompt: "Do it",
        workspaceMode: "current",
        harness: "codex",
        model: "codex:test",
        effort: "high",
      }),
    ).rejects.toThrow('Effort "high" is not available');
    expect(launched).toEqual([]);
  });

  it("rejects a project that is not the Mono's", async () => {
    const { deps } = fixture();
    await expect(
      startWorkflowSession(deps, "mono-1", {
        project: "/code/other",
        prompt: "x",
        workspaceMode: "current",
      }),
    ).rejects.toThrow("Not one of your projects");
  });
});
