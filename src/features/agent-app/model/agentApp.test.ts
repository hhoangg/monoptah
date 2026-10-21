// @vitest-environment happy-dom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { newSession } from "../../sessions/model/session";
import {
  resetHarnessModelOverlays,
  setHarnessModels,
} from "../../sessions/model/models";
import {
  loadSessionFolders,
  saveSessionFolders,
} from "../../sessions/model/sessionFolders";
import type { Note } from "../../notes";
import type { Artifact } from "../../artifacts/artifacts";
import type { Worktree } from "../../source-control/model/worktrees";
import type { MonoLaunch } from "../../monos/model/monoLaunches";
import { handleAgentApp, notePreview, canAccessAgentAppProject, completionChatId, type AgentAppHost } from "./agentApp";

vi.mock("../../../integrations/harness/core/availability", () => ({
  isHarnessAvailable: (id: string) => id === "codex",
}));

const note: Note = {
  id: "n1",
  slug: "plan",
  title: "Plan",
  body: "First paragraph.\n\nSecond paragraph.\n\nThird paragraph should stay out of list.",
  tags: ["work"],
  slugPending: false,
  createdAt: 1,
  updatedAt: 2,
};
const artifact: Artifact = {
  id: "artifact-1",
  kind: "document",
  title: note.title,
  body: note.body,
  createdAt: 1,
  updatedAt: 2,
};
const featureWorktree: Worktree = {
  path: "/tmp/project-worktrees/feature",
  branch: "feature",
  head: "abc123",
  isMain: false,
  locked: false,
  prunable: false,
  missing: false,
  dirty: false,
  unpushed: 0,
  sessionIds: [],
};

const storedValues = new Map<string, string>();
vi.stubGlobal("localStorage", {
  clear: () => storedValues.clear(),
  getItem: (key: string) => storedValues.get(key) ?? null,
  setItem: (key: string, value: string) => storedValues.set(key, value),
  removeItem: (key: string) => storedValues.delete(key),
});

beforeEach(() => {
  localStorage.clear();
  setHarnessModels("codex", [
    {
      id: "codex:test",
      harness: "codex",
      name: "Test model",
      settings: [
        {
          id: "effort",
          label: "Effort",
          kind: "select",
          value: "medium",
          options: [
            { value: "medium", label: "Medium" },
            { value: "high", label: "High" },
          ],
        },
      ],
    },
  ]);
});

afterEach(() => {
  resetHarnessModelOverlays();
});

function fixture() {
  const source = newSession("codex", "/tmp/project", "codex:test");
  source.id = "lead";
  const host: AgentAppHost = {
    start: vi.fn(async () => {}),
    sessions: vi.fn(async () => [
      {
        id: "other",
        title: "Other",
        harness: "codex",
        model: "codex:test",
        busy: false,
        hasDraft: false,
        archived: false,
      },
    ]),
    session: vi.fn(async (id) =>
      id === "other" ? { ...newSession("codex", source.cwd), id } : null,
    ),
    send: vi.fn(async () => ({ alreadySubmitted: false })),
    draft: vi.fn(async () => ({ alreadySaved: false, draft: true })),
    stop: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
    worktrees: vi.fn(async () => ({
      worktrees: [
        { ...featureWorktree },
        { ...featureWorktree, path: source.cwd, isMain: true, branch: "main" },
      ],
      defaultRoot: "/tmp/project-worktrees",
    })),
    createWorktree: vi.fn(async () => ({ ...featureWorktree })),
    notes: vi.fn(async () => [note]),
    note: vi.fn(async (id) => (id === note.id ? note : null)),
    saveNote: vi.fn(async (input) => ({ ...note, ...input })),
    isMono: () => false,
  };
  return { source, host };
}

describe("agent app commands", () => {
  it.each([undefined, true, false])(
    "uses the Mono's sidebar preference for both submitted and draft sessions: %s",
    async (showStartedSessionsInSidebar) => {
      const { source, host } = fixture();
      host.isMono = () => true;
      host.monoOf = () => ({
        id: "mono",
        projects: [source.cwd],
        showStartedSessionsInSidebar,
      });
      for (const draft of [false, true]) {
        await handleAgentApp(source, `launch-${draft}`, "sessions.start", {
          prompt: "Review the project",
          draft,
          notifyOnComplete: false,
        }, host);
        const launch = vi.mocked(host.start).mock.calls.at(-1)![0];
        expect(launch.sidebarHidden).toBe(
          showStartedSessionsInSidebar === true ? undefined : true,
        );
      }
    },
  );

  it("keeps sessions a regular /operator thread starts in the sidebar", async () => {
    const { source, host } = fixture();
    await handleAgentApp(source, "operator-launch", "sessions.start", {
      prompt: "Write an article",
      notifyOnComplete: false,
    }, host);
    const launch = vi.mocked(host.start).mock.calls.at(-1)![0];
    expect(launch.sidebarHidden).toBeUndefined();
  });

  it("uses the owning Mono's preference when a habit starts a session", async () => {
    const { source, host } = fixture();
    host.isHabitRun = () => true;
    host.monoOf = () => ({
      id: "mono",
      projects: [source.cwd],
      showStartedSessionsInSidebar: false,
    });
    await handleAgentApp(source, "habit-launch", "sessions.start", {
      prompt: "Review the project",
      notifyOnComplete: false,
    }, host);
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({ sidebarHidden: true }),
      "app-lead-habit-launch",
    );
  });

  it("files a Mono's sessions into a folder named after it", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    host.monoOf = () => ({
      id: "mono",
      projects: [source.cwd],
      showStartedSessionsInSidebar: true,
      folder: { name: "Captain Jack", color: "#5b8def" },
    });
    for (const request of ["first", "second"])
      await handleAgentApp(source, request, "sessions.start", {
        prompt: "Review the project",
        notifyOnComplete: false,
      }, host);
    expect(loadSessionFolders(source.cwd)).toEqual([
      expect.objectContaining({
        name: "Captain Jack",
        customColor: "#5b8def",
        sessionIds: ["app-lead-first", "app-lead-second"],
      }),
    ]);
  });

  it("leaves a hidden Mono session out of its folder", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    host.monoOf = () => ({
      id: "mono",
      projects: [source.cwd],
      showStartedSessionsInSidebar: false,
      folder: { name: "Captain Jack", color: "#5b8def" },
    });
    await handleAgentApp(source, "hidden", "sessions.start", {
      prompt: "Review the project",
      notifyOnComplete: false,
    }, host);
    expect(loadSessionFolders(source.cwd)).toEqual([]);
  });

  describe.each([
    ["sessions.stop", "stopped"],
    ["sessions.archive", "archived"],
    ["sessions.delete", "deleted"],
  ] as const)("%s", (action, resultField) => {
    it("manages a running session in the caller's project", async () => {
      const { source, host } = fixture();
      vi.mocked(host.session).mockResolvedValue({
        ...newSession("codex", source.cwd),
        id: "other",
        busy: true,
      });
      expect(
        await handleAgentApp(
          source,
          "manage",
          action,
          { sessionId: "other" },
          host,
        ),
      ).toEqual({ sessionId: "other", [resultField]: true });
      if (action === "sessions.stop") {
        expect(host.stop).toHaveBeenCalledExactlyOnceWith("other");
        expect(host.remove).not.toHaveBeenCalled();
      } else {
        expect(host.remove).toHaveBeenCalledExactlyOnceWith(
          "other",
          action === "sessions.archive" ? "archive" : "delete",
        );
        expect(host.stop).not.toHaveBeenCalled();
      }
    });

    it("lets a Mono manage sessions in a selected assigned project", async () => {
      const { source, host } = fixture();
      source.cwd = "/home/user";
      host.isMono = (id) => id === source.id;
      host.monoOf = () => ({
        id: "mono",
        projects: ["/code/app", "/code/site"],
      });
      vi.mocked(host.session).mockResolvedValue({
        ...newSession("codex", "/code/site"),
        id: "other",
      });
      expect(
        await handleAgentApp(
          source,
          "manage",
          action,
          {
            sessionId: "other",
            project: "site",
          },
          host,
        ),
      ).toEqual({ sessionId: "other", [resultField]: true });
      expect(host.sessions).toHaveBeenCalledWith("/code/site");
      await expect(
        handleAgentApp(
          source,
          "outside",
          action,
          {
            sessionId: "other",
            project: "/code/unassigned",
          },
          host,
        ),
      ).rejects.toThrow("Not one of your projects");
      await expect(
        handleAgentApp(
          source,
          "wrong-project",
          action,
          {
            sessionId: "other",
            project: "/code/app",
          },
          host,
        ),
      ).rejects.toThrow("not found in this project");
      expect(
        vi.mocked(host.stop).mock.calls.length +
          vi.mocked(host.remove).mock.calls.length,
      ).toBe(1);
    });

    it("rejects the caller, missing sessions and sessions outside the selected project", async () => {
      const { source, host } = fixture();
      await expect(
        handleAgentApp(source, "self", action, { sessionId: source.id }, host),
      ).rejects.toThrow("calling session");
      await expect(
        handleAgentApp(
          source,
          "missing",
          action,
          { sessionId: "missing" },
          host,
        ),
      ).rejects.toThrow("not found in this project");
      vi.mocked(host.session).mockResolvedValue({
        ...newSession("codex", "/another-project"),
        id: "other",
      });
      await expect(
        handleAgentApp(source, "outside", action, { sessionId: "other" }, host),
      ).rejects.toThrow("not found in this project");
      expect(host.stop).not.toHaveBeenCalled();
      expect(host.remove).not.toHaveBeenCalled();
    });

    it("does not manage Mono chats, habit runs or orchestration workers", async () => {
      const { source, host } = fixture();
      host.isMono = () => true;
      await expect(
        handleAgentApp(source, "mono", action, { sessionId: "other" }, host),
      ).rejects.toThrow("Only regular project sessions");
      host.isMono = () => false;
      host.isHabitRun = () => true;
      await expect(
        handleAgentApp(source, "habit", action, { sessionId: "other" }, host),
      ).rejects.toThrow("Only regular project sessions");
      host.isHabitRun = () => false;
      vi.mocked(host.session).mockResolvedValue({
        ...newSession("codex", source.cwd),
        id: "other",
        orchestrationLeadId: "orchestrator",
      });
      await expect(
        handleAgentApp(source, "worker", action, { sessionId: "other" }, host),
      ).rejects.toThrow("Only regular project sessions");
      expect(host.stop).not.toHaveBeenCalled();
      expect(host.remove).not.toHaveBeenCalled();
    });

    it("rejects invalid request IDs and unsupported fields before making changes", async () => {
      const { source, host } = fixture();
      await expect(
        handleAgentApp(source, "bad/id", action, { sessionId: "other" }, host),
      ).rejects.toThrow("Invalid request ID");
      await expect(
        handleAgentApp(
          source,
          "extra",
          action,
          {
            sessionId: "other",
            deleteWorktree: true,
          },
          host,
        ),
      ).rejects.toThrow("Unknown");
      expect(host.stop).not.toHaveBeenCalled();
      expect(host.remove).not.toHaveBeenCalled();
    });

    it("reports lifecycle failures instead of claiming success", async () => {
      const { source, host } = fixture();
      vi.mocked(host.stop).mockRejectedValue(new Error("operation failed"));
      vi.mocked(host.remove).mockRejectedValue(new Error("operation failed"));
      await expect(
        handleAgentApp(source, "failed", action, { sessionId: "other" }, host),
      ).rejects.toThrow("operation failed");
    });
  });

  it("allows session inspection in a Mono's projects when its chat lives at home", () => {
    const { source } = fixture();
    source.cwd = "/home/user";
    const projects = ["/code/app", "/code/site"];
    expect(canAccessAgentAppProject(source, "/code/app/", projects)).toBe(true);
    expect(canAccessAgentAppProject(source, "/code/site", projects)).toBe(true);
    expect(canAccessAgentAppProject(source, source.cwd, projects)).toBe(false);
    expect(canAccessAgentAppProject(source, "/code/other", projects)).toBe(
      false,
    );
    expect(canAccessAgentAppProject(source, "/code/app", [])).toBe(false);
    expect(canAccessAgentAppProject(source, source.cwd)).toBe(true);
    expect(canAccessAgentAppProject(source, "/code/app")).toBe(false);
  });

  it.each([undefined, true])("reports completion of Mono launches with notifyOnComplete=%s", async (notifyOnComplete) => {
    const { source, host } = fixture();
    host.isMono = (id) => id === source.id;
    expect(
      await handleAgentApp(
        source,
        "monitored",
        "sessions.start",
        {
          prompt: "Review the API",
          ...(notifyOnComplete === undefined ? {} : { notifyOnComplete }),
        },
        host,
      ),
    ).toMatchObject({
      id: "app-lead-monitored",
      submitted: true,
      notifyOnComplete: true,
    });
    expect(host.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ prompt: "Review the API" }),
      "app-lead-monitored",
      undefined,
      "lead",
    );
    const optedOut = await handleAgentApp(
      source,
      "ordinary",
      "sessions.start",
      { prompt: "Review", notifyOnComplete: false },
      host,
    );
    expect(optedOut).not.toHaveProperty("notifyOnComplete");
    expect(host.start).toHaveBeenLastCalledWith(
      expect.anything(),
      "app-lead-ordinary",
    );
  });

  it("does not monitor a Mono's unsent draft by default", async () => {
    const { source, host } = fixture();
    host.isMono = (id) => id === source.id;
    const result = await handleAgentApp(
      source,
      "draft",
      "sessions.start",
      { prompt: "Review the API", draft: true },
      host,
    );
    expect(result).toMatchObject({ submitted: false, draft: true });
    expect(result).not.toHaveProperty("notifyOnComplete");
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({ draft: true }),
      "app-lead-draft",
    );
  });

  it.each([false, true])("leaves non-Mono launches unmonitored when habitRun=%s", async (habitRun) => {
    const { source, host } = fixture();
    host.isHabitRun = () => habitRun;
    const result = await handleAgentApp(
      source,
      "ordinary",
      "sessions.start",
      { prompt: "Review the API" },
      host,
    );
    expect(result).not.toHaveProperty("notifyOnComplete");
    expect(host.start).toHaveBeenCalledWith(
      expect.anything(),
      "app-lead-ordinary",
    );
  });

  it("reports completion of a sent follow-up to its calling Mono", async () => {
    const { source, host } = fixture();
    host.isMono = (id) => id === source.id;
    expect(
      await handleAgentApp(
        source,
        "monitored-send",
        "sessions.send",
        {
          sessionId: "other",
          prompt: "Fix the findings",
          notifyOnComplete: true,
        },
        host,
      ),
    ).toMatchObject({ submitted: true, notifyOnComplete: true });
    expect(host.send).toHaveBeenCalledWith(
      "other",
      "Fix the findings",
      "app-lead-monitored-send",
      "lead",
    );
  });

  it.each([
    { reveal: undefined },
    { reveal: true },
    { reveal: true, notifyOnComplete: true },
    { reveal: true, draft: true },
    { reveal: true, placement: "right", besideSessionId: "other" },
  ])("keeps Mono-launched sessions in the background for %j", async (options) => {
    const { source, host } = fixture();
    host.isMono = (id) => id === source.id;
    await handleAgentApp(
      source,
      "background",
      "sessions.start",
      { prompt: "Review the API", ...options },
      host,
    );
    expect(vi.mocked(host.start).mock.calls[0][0]).toMatchObject({
      prompt: "Review the API",
      reveal: false,
    });
  });

  it("reports completion by default through split placement", async () => {
    const { source, host } = fixture();
    host.isMono = (id) => id === source.id;
    await handleAgentApp(
      source,
      "monitored-split",
      "sessions.start",
      {
        prompt: "Review",
        placement: "right",
      },
      host,
    );
    expect(host.start).toHaveBeenCalledWith(
      expect.anything(),
      "app-lead-monitored-split",
      {
        direction: "right",
        besideSessionId: "lead",
      },
      "lead",
    );
  });

  it.each(["sessions.start", "sessions.send"])(
    "validates completion notification options for %s before submission",
    async (action) => {
      const { source, host } = fixture();
      const input = {
        prompt: "Review",
        ...(action === "sessions.send" ? { sessionId: "other" } : {}),
      };
      await expect(
        handleAgentApp(
          source,
          "wrong-type",
          action,
          { ...input, notifyOnComplete: "yes" },
          host,
        ),
      ).rejects.toThrow("must be a boolean");
      await expect(
        handleAgentApp(
          source,
          "not-mono",
          action,
          { ...input, notifyOnComplete: true },
          host,
        ),
      ).rejects.toThrow("only available in a Mono");
      expect(host.start).not.toHaveBeenCalled();
      expect(host.send).not.toHaveBeenCalled();
      await handleAgentApp(
        source,
        "disabled",
        action,
        { ...input, notifyOnComplete: false },
        host,
      );
    },
  );

  it("rejects completion reports for an unsent draft", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    await expect(
      handleAgentApp(
        source,
        "draft-notify",
        "sessions.start",
        {
          prompt: "Review",
          draft: true,
          notifyOnComplete: true,
        },
        host,
      ),
    ).rejects.toThrow("unsent draft");
    expect(host.start).not.toHaveBeenCalled();
  });

  it("lets a Mono page its own chat without requiring a project", async () => {
    const { source, host } = fixture();
    host.isMono = (id) => id === source.id;
    host.readConversation = vi.fn(async () => ({ sessionId: source.id, title: "Mono", busy: false, hasDraft: false, turns: [], nextBefore: "older" }));
    const options = { before: "cursor", limit: 3, maxChars: 1200 };
    expect(await handleAgentApp(source, "own-chat", "sessions.read", { sessionId: source.id, ...options }, host)).toMatchObject({ nextBefore: "older" });
    expect(host.readConversation).toHaveBeenCalledWith(source, options);
    expect(host.sessions).not.toHaveBeenCalled();
  });

  it("reads a listed project session in bounded pages", async () => {
    const { source, host } = fixture();
    const result = await handleAgentApp(
      source,
      "read-1",
      "sessions.read",
      { sessionId: "other" },
      host,
    );
    expect(result).toMatchObject({ sessionId: "other", turns: [] });
    await expect(
      handleAgentApp(
        source,
        "read-2",
        "sessions.read",
        { sessionId: "missing" },
        host,
      ),
    ).rejects.toThrow("not found in this project");
  });

  it("sends a follow-up only to a listed idle session", async () => {
    const { source, host } = fixture();
    expect(
      await handleAgentApp(
        source,
        "send-1",
        "sessions.send",
        { sessionId: "other", prompt: "Continue the review" },
        host,
      ),
    ).toMatchObject({ sessionId: "other", submitted: true });
    expect(host.send).toHaveBeenCalledWith(
      "other",
      "Continue the review",
      "app-lead-send-1",
    );
    await expect(
      handleAgentApp(
        source,
        "send-2",
        "sessions.send",
        { sessionId: "lead", prompt: "loop" },
        host,
      ),
    ).rejects.toThrow("current conversation");
    await expect(
      handleAgentApp(
        source,
        "send-3",
        "sessions.send",
        { sessionId: "missing", prompt: "hello" },
        host,
      ),
    ).rejects.toThrow("not found in this project");
    expect(host.send).toHaveBeenCalledTimes(1);
  });

  it("saves an unsent draft in another listed project session", async () => {
    const { source, host } = fixture();
    expect(
      await handleAgentApp(
        source,
        "draft-1",
        "sessions.draft",
        { sessionId: "other", prompt: "Review this idea later" },
        host,
      ),
    ).toMatchObject({ sessionId: "other", saved: true, draft: true });
    expect(host.draft).toHaveBeenCalledWith(
      "other",
      "Review this idea later",
      "app-lead-draft-1",
    );
    await expect(
      handleAgentApp(
        source,
        "draft-2",
        "sessions.draft",
        { sessionId: "lead", prompt: "Not here" },
        host,
      ),
    ).rejects.toThrow("composer");
    await expect(
      handleAgentApp(
        source,
        "draft-3",
        "sessions.draft",
        { sessionId: "missing", prompt: "Not there" },
        host,
      ),
    ).rejects.toThrow("not found in this project");
    expect(host.draft).toHaveBeenCalledTimes(1);
  });

  it("does not let app-supplied prompts enable Operator in another session", async () => {
    const { source, host } = fixture();
    for (const action of [
      "sessions.send",
      "sessions.draft",
      "sessions.start",
    ]) {
      for (const prompt of [
        "/operator list notes",
        "/mono list notes",
        "  /MONOCODE list notes",
      ]) {
        await expect(
          handleAgentApp(
            source,
            "blocked",
            action,
            action === "sessions.start"
              ? { prompt }
              : { sessionId: "other", prompt },
            host,
          ),
        ).rejects.toThrow("cannot enable /operator");
      }
    }
    expect(host.send).not.toHaveBeenCalled();
    expect(host.draft).not.toHaveBeenCalled();
    expect(host.start).not.toHaveBeenCalled();

    await handleAgentApp(
      source,
      "ordinary",
      "sessions.send",
      { sessionId: "other", prompt: "Explain the /operator command" },
      host,
    );
    expect(host.send).toHaveBeenCalledOnce();
  });

  it("starts a submitted tab with explicit model, effort, permissions and workspace", async () => {
    const { source, host } = fixture();
    const result = await handleAgentApp(
      source,
      "request-1",
      "sessions.start",
      {
        prompt: "Inspect the API",
        model: "codex:test",
        effort: "high",
        runtimeMode: "auto-accept-edits",
        workspaceMode: "worktree",
        reveal: true,
      },
      host,
    );
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({
        prompt: "Inspect the API",
        cwd: "/tmp/project",
        model: "codex:test",
        modelSettings: { effort: "high" },
        runtimeMode: "auto-accept-edits",
        workspaceMode: "worktree",
        reveal: true,
      }),
      "app-lead-request-1",
    );
    expect(result).toMatchObject({ id: "app-lead-request-1", submitted: true });
  });

  describe("a project with no commits", () => {
    const unborn = (host: AgentAppHost, cwd: string) => {
      host.worktrees = vi.fn(async () => ({
        worktrees: [
          { ...featureWorktree, path: cwd, isMain: true, branch: "main", head: "0".repeat(40) },
        ],
        defaultRoot: "/tmp/project-worktrees",
      }));
    };

    it("rejects a worktree launch before starting anything, filing a folder or recording it", async () => {
      const { source, host } = fixture();
      unborn(host, source.cwd);
      const record = vi.fn();
      host.launches = { get: () => undefined, record };
      await expect(
        handleAgentApp(
          source,
          "no-commit",
          "sessions.start",
          {
            prompt: "Add a file",
            workspaceMode: "worktree",
            folderName: "Tickets",
          },
          host,
        ),
      ).rejects.toThrow("has no commits yet");
      expect(host.start).not.toHaveBeenCalled();
      expect(record).not.toHaveBeenCalled();
      expect(loadSessionFolders(source.cwd)).toEqual([]);
    });

    it("still starts in the current checkout", async () => {
      const { source, host } = fixture();
      unborn(host, source.cwd);
      await handleAgentApp(
        source,
        "current",
        "sessions.start",
        { prompt: "Add a file", workspaceMode: "current" },
        host,
      );
      expect(host.start).toHaveBeenCalledOnce();
    });
  });

  it("lists project worktrees and starts on a selected existing checkout", async () => {
    const { source, host } = fixture();
    source.worktreeCwd = "/tmp/project-worktrees/other";
    const listed = await handleAgentApp(
      source,
      "list",
      "worktrees.list",
      {},
      host,
    );
    expect((listed as { worktrees: Worktree[] }).worktrees[0]).toMatchObject({
      branch: "feature",
    });
    expect(host.worktrees).toHaveBeenCalledWith(source.cwd);

    await handleAgentApp(
      source,
      "feature",
      "sessions.start",
      {
        prompt: "Review feature",
        worktreeCwd: featureWorktree.path,
      },
      host,
    );
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({ worktreeCwd: featureWorktree.path }),
      "app-lead-feature",
    );
    await handleAgentApp(
      source,
      "main",
      "sessions.start",
      {
        prompt: "Review main",
        worktreeCwd: source.cwd,
      },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ worktreeCwd: undefined }),
      "app-lead-main",
    );
  });

  it("rejects unavailable or conflicting worktree choices before launching", async () => {
    const { source, host } = fixture();
    for (const input of [
      { prompt: "A", worktreeCwd: "/tmp/other-repo" },
      {
        prompt: "A",
        workspaceMode: "worktree",
        worktreeCwd: featureWorktree.path,
      },
      { prompt: "A", worktreeBase: "main", worktreeCwd: featureWorktree.path },
    ]) {
      await expect(
        handleAgentApp(source, "invalid", "sessions.start", input, host),
      ).rejects.toThrow();
    }
    vi.mocked(host.worktrees).mockResolvedValueOnce({
      worktrees: [{ ...featureWorktree, missing: true }],
      defaultRoot: "/tmp/project-worktrees",
    });
    await expect(
      handleAgentApp(
        source,
        "missing",
        "sessions.start",
        { prompt: "A", worktreeCwd: featureWorktree.path },
        host,
      ),
    ).rejects.toThrow("unavailable in this project");
    expect(host.start).not.toHaveBeenCalled();
  });

  it.each([
    ["/tmp/project", "/tmp/project-worktrees/source"],
    ["/tmp/other", undefined],
  ])("inherits a worktree only when launching in the source project: %s", async (project, expectedWorktree) => {
    const { source, host } = fixture();
    source.worktreeCwd = "/tmp/project-worktrees/source";
    host.isMono = () => true;
    host.monoOf = () => ({ id: "mono", projects: [source.cwd, "/tmp/other"] });
    await handleAgentApp(source, "launch", "sessions.start", {
      prompt: "Review the project",
      project,
      notifyOnComplete: false,
    }, host);
    const launch = vi.mocked(host.start).mock.calls[0][0];
    expect(launch.cwd).toBe(project);
    expect(launch.worktreeCwd).toBe(expectedWorktree);
  });

  it("validates an explicit worktree in the Mono's selected project", async () => {
    const { source, host } = fixture();
    source.worktreeCwd = "/tmp/project-worktrees/source";
    host.isMono = () => true;
    host.monoOf = () => ({ id: "mono", projects: [source.cwd, "/tmp/other"] });
    const chosen = "/tmp/other-worktrees/feature";
    vi.mocked(host.worktrees).mockResolvedValue({
      worktrees: [{ ...featureWorktree, path: chosen }],
      defaultRoot: "/tmp/other-worktrees",
    });
    await handleAgentApp(source, "launch", "sessions.start", {
      prompt: "Review the feature",
      project: "/tmp/other",
      worktreeCwd: chosen,
      notifyOnComplete: false,
    }, host);
    expect(host.worktrees).toHaveBeenCalledWith("/tmp/other");
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({ cwd: "/tmp/other", worktreeCwd: chosen }),
      "app-lead-launch",
    );
  });

  it("creates a worktree on a named new or existing branch", async () => {
    const { source, host } = fixture();
    expect(
      await handleAgentApp(
        source,
        "new",
        "worktrees.create",
        {
          branch: "feature",
          base: "origin/main",
        },
        host,
      ),
    ).toMatchObject({ path: featureWorktree.path });
    expect(host.createWorktree).toHaveBeenCalledWith(
      source.cwd,
      "feature",
      "origin/main",
      false,
    );
    await handleAgentApp(
      source,
      "existing",
      "worktrees.create",
      {
        branch: "feature",
        existing: true,
      },
      host,
    );
    expect(host.createWorktree).toHaveBeenLastCalledWith(
      source.cwd,
      "feature",
      "HEAD",
      true,
    );
    await expect(
      handleAgentApp(
        source,
        "bad",
        "worktrees.create",
        {
          branch: "feature",
          base: "main",
          existing: true,
        },
        host,
      ),
    ).rejects.toThrow("base cannot be set");
    expect(host.createWorktree).toHaveBeenCalledTimes(2);
  });

  it("starts a pane beside the caller or another session in either direction", async () => {
    const { source, host } = fixture();
    await handleAgentApp(
      source,
      "right",
      "sessions.start",
      { prompt: "Inspect the API", placement: "right", draft: true },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ draft: true }),
      "app-lead-right",
      { direction: "right", besideSessionId: "lead" },
    );
    await handleAgentApp(
      source,
      "down",
      "sessions.start",
      {
        prompt: "Review the UI",
        placement: "down",
        besideSessionId: "app-lead-right",
      },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.anything(),
      "app-lead-down",
      { direction: "down", besideSessionId: "app-lead-right" },
    );
  });

  it("rejects invalid pane placement before starting", async () => {
    const { source, host } = fixture();
    for (const input of [
      { prompt: "A", placement: "left" },
      { prompt: "A", besideSessionId: "other" },
      { prompt: "A", placement: "down", besideSessionId: 42 },
    ]) {
      await expect(
        handleAgentApp(source, "invalid", "sessions.start", input, host),
      ).rejects.toThrow();
    }
    expect(host.start).not.toHaveBeenCalled();
  });

  it("inherits the caller's permission mode unless start overrides it", async () => {
    const { source, host } = fixture();
    source.runtimeMode = "auto";
    await handleAgentApp(
      source,
      "inherited-mode",
      "sessions.start",
      { prompt: "Review this" },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ runtimeMode: "auto" }),
      "app-lead-inherited-mode",
    );
    await handleAgentApp(
      source,
      "explicit-mode",
      "sessions.start",
      { prompt: "Review this", runtimeMode: "full-access" },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ runtimeMode: "full-access" }),
      "app-lead-explicit-mode",
    );
  });

  it("starts with an unsent draft and can immediately move the new session into a folder", async () => {
    const { source, host } = fixture();
    const result = (await handleAgentApp(
      source,
      "draft-launch",
      "sessions.start",
      {
        prompt: "Test prompt",
        model: "codex:test",
        draft: true,
      },
      host,
    )) as { id: string; submitted: boolean; draft: boolean };
    expect(result).toMatchObject({
      id: "app-lead-draft-launch",
      submitted: false,
      draft: true,
    });
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: "Test prompt", draft: true }),
      result.id,
    );
    host.sessions = vi.fn(async () => [
      {
        id: result.id,
        title: "Test prompt",
        harness: "codex",
        model: "codex:test",
        busy: false,
        hasDraft: true,
      },
    ]);
    const moved = await handleAgentApp(
      source,
      "folder",
      "folders.move",
      { sessionId: result.id, newFolderName: "test" },
      host,
    );
    expect(moved).toMatchObject({ sessionId: result.id, folderName: "test" });
  });

  it("rejects invalid model settings before starting a session", async () => {
    const { source, host } = fixture();
    await expect(
      handleAgentApp(
        source,
        "bad",
        "sessions.start",
        {
          prompt: "Hello",
          effort: "ultra",
        },
        host,
      ),
    ).rejects.toThrow("Invalid model setting effort");
    expect(host.start).not.toHaveBeenCalled();
    await expect(
      handleAgentApp(
        source,
        "bad-draft",
        "sessions.start",
        { prompt: "Hello", draft: "true" },
        host,
      ),
    ).rejects.toThrow("draft must be a boolean");
    await expect(
      handleAgentApp(source, "unexpected", "toString", {}, host),
    ).rejects.toThrow("Unknown app action");
  });

  it("moves an existing project session into a sidebar folder", async () => {
    const { source, host } = fixture();
    saveSessionFolders(source.cwd, [
      {
        id: "folder-1",
        name: "Research",
        sessionIds: ["lead"],
        collapsed: false,
      },
    ]);
    expect(
      await handleAgentApp(
        source,
        "move",
        "folders.move",
        {
          sessionId: "other",
          folderId: "folder-1",
        },
        host,
      ),
    ).toMatchObject({ folderId: "folder-1", sessionId: "other" });
    expect(loadSessionFolders(source.cwd)[0]?.sessionIds).toEqual([
      "lead",
      "other",
    ]);
  });

  it("creates a folder during a move and rejects unknown sessions", async () => {
    const { source, host } = fixture();
    await expect(
      handleAgentApp(
        source,
        "bad-move",
        "folders.move",
        {
          sessionId: "missing",
          newFolderName: "Research",
        },
        host,
      ),
    ).rejects.toThrow("Session was not found");
    const moved = (await handleAgentApp(
      source,
      "new-folder",
      "folders.move",
      {
        sessionId: "other",
        newFolderName: "Research",
      },
      host,
    )) as { folderId: string; folderName: string };
    expect(moved.folderName).toBe("Research");
    expect(loadSessionFolders(source.cwd)[0]).toMatchObject({
      id: moved.folderId,
      sessionIds: ["other"],
    });
  });

  it("lists short note previews and reads one full note on request", async () => {
    const { source, host } = fixture();
    const listed = (await handleAgentApp(
      source,
      "list",
      "notes.list",
      {},
      host,
    )) as {
      notes: Array<{ preview: string; body?: string }>;
    };
    expect(listed.notes[0]?.preview).toBe(
      "First paragraph.\n\nSecond paragraph.",
    );
    expect(listed.notes[0]).not.toHaveProperty("body");
    expect(notePreview("A".repeat(500))).toHaveLength(400);
    expect(
      await handleAgentApp(source, "read", "notes.read", { id: "n1" }, host),
    ).toMatchObject({ body: note.body });
  });

  it("creates a note with source metadata and reuses the same request ID safely", async () => {
    const { source, host } = fixture();
    let created: Note | null = null;
    host.note = vi.fn(async (id) => (id === created?.id ? created : null));
    host.saveNote = vi.fn(async (input) => {
      created = { ...note, ...input };
      return created;
    });
    const input = { body: "# Work plan\n\nNext steps", tags: ["#Work"] };
    const saved = await handleAgentApp(
      source,
      "create-1",
      "notes.write",
      input,
      host,
    );
    expect(saved).toMatchObject({
      id: "app-lead-create-1",
      title: "Work plan",
      body: input.body,
      tags: ["work"],
      sourceSessionId: source.id,
      sourceCwd: source.cwd,
    });
    expect(
      await handleAgentApp(source, "create-1", "notes.write", input, host),
    ).toEqual(saved);
    expect(host.saveNote).toHaveBeenCalledTimes(1);
    await expect(
      handleAgentApp(
        source,
        "create-1",
        "notes.write",
        {
          body: "Different body",
        },
        host,
      ),
    ).rejects.toThrow("Request ID was already used");
  });

  it("saves artifacts separately, returns metadata, and attaches one reference on retries", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    let created: Artifact | null = null;
    host.artifact = vi.fn(async (id) => (id === created?.id ? created : null));
    host.saveArtifact = vi.fn(async (input) => {
      created = { ...artifact, ...input };
      return created;
    });
    host.postArtifact = vi.fn();
    const input = {
      kind: "document",
      title: "PR review",
      body: "# PR review\n\nFull report",
      summary: "Merge queue and blockers",
    };
    const result = await handleAgentApp(
      source,
      "report-1",
      "artifacts.write",
      input,
      host,
    );
    expect(result).toEqual({
      id: "artifact-lead-report-1",
      kind: "document",
      title: input.title,
      summary: input.summary,
      saved: true,
      attached: true,
    });
    expect(result).not.toHaveProperty("body");
    expect(host.saveNote).not.toHaveBeenCalled();
    expect(host.saveArtifact).toHaveBeenCalledWith(
      expect.objectContaining({
        sourceSessionId: source.id,
        body: input.body,
        kind: "document",
      }),
    );
    expect(
      await handleAgentApp(source, "report-1", "artifacts.write", input, host),
    ).toEqual(result);
    expect(host.saveArtifact).toHaveBeenCalledTimes(1);
    await expect(
      handleAgentApp(
        source,
        "report-1",
        "artifacts.write",
        { ...input, body: "Different" },
        host,
      ),
    ).rejects.toThrow("Request ID was already used");
    expect(host.postArtifact).toHaveBeenCalledWith(source.id, {
      id: "artifact-lead-report-1",
      kind: "document",
      title: input.title,
      summary: input.summary,
    });
  });

  it("revises a document without changing omitted fields and refuses note IDs", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    host.artifact = vi.fn(async (id) =>
      id === "artifact-1" ? { ...artifact, id } : null,
    );
    host.saveArtifact = vi.fn(async (input) => ({ ...artifact, ...input }));
    host.postArtifact = vi.fn();
    await handleAgentApp(
      source,
      "edit",
      "artifacts.write",
      { id: "artifact-1", body: "Revised" },
      host,
    );
    expect(host.saveArtifact).toHaveBeenCalledWith({
      id: "artifact-1",
      title: note.title,
      body: "Revised",
      kind: "document",
    });
    await expect(
      handleAgentApp(
        source,
        "bad",
        "artifacts.write",
        { id: note.id, body: "Oops" },
        host,
      ),
    ).rejects.toThrow("Artifact was not found");
    expect(host.saveNote).not.toHaveBeenCalled();
  });

  it("allows habit artifacts and recovers an attachment failure without saving twice", async () => {
    const { source, host } = fixture();
    host.isHabitRun = () => true;
    let created: Artifact | null = null;
    host.artifact = vi.fn(async () => created);
    host.saveArtifact = vi.fn(async (input) => {
      created = { ...artifact, ...input };
      return created;
    });
    host.postArtifact = vi
      .fn()
      .mockRejectedValueOnce(new Error("Attachment failed"))
      .mockResolvedValue(undefined);
    const input = { body: "# Habit report\n\nDetails" };
    await expect(
      handleAgentApp(source, "report", "artifacts.write", input, host),
    ).rejects.toThrow("Attachment failed");
    await expect(
      handleAgentApp(source, "report", "artifacts.write", input, host),
    ).resolves.toMatchObject({ attached: true });
    expect(host.saveArtifact).toHaveBeenCalledTimes(1);
  });

  it("restricts artifact actions to Monos and lists only document metadata", async () => {
    const { source, host } = fixture();
    await expect(
      handleAgentApp(
        source,
        "report",
        "artifacts.write",
        { body: "Report" },
        host,
      ),
    ).rejects.toThrow("Only a Mono");
    host.isMono = () => true;
    host.artifact = vi.fn(async () => artifact);
    host.artifacts = vi.fn(async () => [{ ...artifact, id: "artifact-1" }]);
    const result = await handleAgentApp(
      source,
      "list",
      "artifacts.list",
      {},
      host,
    );
    expect(result).toEqual({
      total: 1,
      offset: 0,
      artifacts: [
        {
          id: "artifact-1",
          kind: "document",
          title: artifact.title,
          updatedAt: 2,
        },
      ],
    });
    expect(host.notes).not.toHaveBeenCalled();
    await expect(
      handleAgentApp(
        source,
        "list-bad",
        "artifacts.list",
        { limit: 101 },
        host,
      ),
    ).rejects.toThrow("limit");
    await expect(
      handleAgentApp(
        source,
        "read",
        "artifacts.read",
        { id: "artifact-1" },
        host,
      ),
    ).resolves.toMatchObject({ body: note.body });
  });

  it("saves an html visual with its kind and posts it to the reply", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    host.artifact = vi.fn(async () => null);
    host.saveArtifact = vi.fn(async (artifact) => ({
      ...artifact,
      createdAt: 1,
      updatedAt: 1,
    }));
    host.postArtifact = vi.fn();
    await expect(
      handleAgentApp(
        source,
        "visual",
        "artifacts.write",
        { kind: "html", title: "Build times", body: "<div>chart</div>" },
        host,
      ),
    ).resolves.toMatchObject({ kind: "html", title: "Build times" });
    expect(host.saveArtifact).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "html", body: "<div>chart</div>" }),
    );
    expect(host.postArtifact).toHaveBeenCalledWith(
      source.id,
      expect.objectContaining({ kind: "html" }),
    );
  });

  it("rejects unsupported artifact kinds before saving or posting a card", async () => {
    const { source, host } = fixture();
    host.isMono = () => true;
    host.artifact = vi.fn(async () => null);
    host.artifacts = vi.fn(async () => []);
    host.saveArtifact = vi.fn();
    host.postArtifact = vi.fn();
    for (const kind of ["code", "preview", "note", null]) {
      await expect(
        handleAgentApp(
          source,
          "bad",
          "artifacts.write",
          {
            kind,
            body: "Content",
          },
          host,
        ),
      ).rejects.toThrow("Unsupported artifact kind");
    }
    await expect(
      handleAgentApp(
        source,
        "bad",
        "artifacts.list",
        {
          kind: "code",
        },
        host,
      ),
    ).rejects.toThrow("Unsupported artifact kind");
    expect(host.saveArtifact).not.toHaveBeenCalled();
    expect(host.postArtifact).not.toHaveBeenCalled();
  });

  it("edits only supplied note fields and refuses missing or malformed notes", async () => {
    const { source, host } = fixture();
    const changed = await handleAgentApp(
      source,
      "edit-1",
      "notes.write",
      {
        id: "n1",
        body: "Updated body",
      },
      host,
    );
    expect(changed).toMatchObject({
      id: "n1",
      title: note.title,
      body: "Updated body",
      tags: note.tags,
    });
    expect(host.saveNote).toHaveBeenCalledWith({
      id: "n1",
      title: note.title,
      body: "Updated body",
      tags: note.tags,
    });
    await expect(
      handleAgentApp(
        source,
        "edit-2",
        "notes.write",
        {
          id: "missing",
          body: "x",
        },
        host,
      ),
    ).rejects.toThrow("Note was not found");
    await expect(
      handleAgentApp(
        source,
        "edit-3",
        "notes.write",
        {
          id: "n1",
        },
        host,
      ),
    ).rejects.toThrow("Supply title, body or tags");
    await expect(
      handleAgentApp(
        source,
        "edit-4",
        "notes.write",
        {
          id: "n1",
          tags: "work",
        },
        host,
      ),
    ).rejects.toThrow("tags must be an array");
    await expect(
      handleAgentApp(
        source,
        "create-2",
        "notes.write",
        {
          title: "Empty",
        },
        host,
      ),
    ).rejects.toThrow("body is required");
  });
});

function listSessions(host: AgentAppHost, cwd: string, ids: string[]) {
  const listing = (id: string) => ({
    id,
    title: id,
    harness: "codex" as const,
    model: "codex:test",
    busy: false,
    hasDraft: false,
    archived: false,
  });
  host.sessions = vi.fn(async () => ids.map(listing));
  host.session = vi.fn(async (id) =>
    ids.includes(id) ? { ...newSession("codex", cwd), id } : null,
  );
}

function monoFixture(
  mono: Partial<NonNullable<ReturnType<NonNullable<AgentAppHost["monoOf"]>>>> = {},
) {
  const { source, host } = fixture();
  host.isMono = (id) => id === source.id;
  host.monoOf = () => ({
    id: "mono",
    projects: [source.cwd],
    showStartedSessionsInSidebar: true,
    ...mono,
  });
  const launches = new Map<string, MonoLaunch>();
  host.launches = {
    record: vi.fn((sessionId, monoId, parented) => {
      launches.set(sessionId, { monoId, parented });
    }),
    get: (sessionId) => launches.get(sessionId),
  };
  return { source, host, launches };
}

function habitFixture(sessionId: string | null = "mono-chat") {
  const { source, host } = fixture();
  source.id = "run";
  host.isHabitRun = (id) => id === source.id;
  host.monoOf = () => ({
    id: "mono",
    projects: [source.cwd],
    showStartedSessionsInSidebar: true,
    ...(sessionId ? { sessionId } : {}),
  });
  return { source, host };
}

describe("habit runs reporting to their Mono", () => {
  it("reports a habit run's launches to the Mono's chat, one by one", async () => {
    const { source, host } = habitFixture();
    const result = await handleAgentApp(
      source,
      "r1",
      "sessions.start",
      { prompt: "Fix the ticket" },
      host,
    );
    expect(result).toMatchObject({ id: "app-run-r1", notifyOnComplete: true });
    expect(host.start).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: "Fix the ticket" }),
      "app-run-r1",
      undefined,
      "mono-chat",
      "each",
    );
  });

  it("gives a habit run per-session delivery even when it asks for a group", async () => {
    const { source, host } = habitFixture();
    await handleAgentApp(
      source,
      "r1",
      "sessions.start",
      { prompt: "Fix", completionDelivery: "group" },
      host,
    );
    expect(host.start).toHaveBeenCalledWith(
      expect.anything(),
      "app-run-r1",
      undefined,
      "mono-chat",
      "each",
    );
  });

  it("leaves an explicit opt-out and a draft of a habit run unmonitored", async () => {
    const { source, host } = habitFixture();
    const optedOut = await handleAgentApp(
      source,
      "r1",
      "sessions.start",
      { prompt: "Fix", notifyOnComplete: false },
      host,
    );
    const draft = await handleAgentApp(
      source,
      "r2",
      "sessions.start",
      { prompt: "Fix", draft: true },
      host,
    );
    expect(optedOut).not.toHaveProperty("notifyOnComplete");
    expect(draft).not.toHaveProperty("notifyOnComplete");
    expect(host.start).toHaveBeenNthCalledWith(1, expect.anything(), "app-run-r1");
    expect(host.start).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ draft: true }),
      "app-run-r2",
    );
  });

  it("treats a habit run with no Mono chat as unable to be notified", async () => {
    const { source, host } = habitFixture(null);
    const result = await handleAgentApp(
      source,
      "r1",
      "sessions.start",
      { prompt: "Fix" },
      host,
    );
    expect(result).not.toHaveProperty("notifyOnComplete");
    expect(host.start).toHaveBeenCalledWith(expect.anything(), "app-run-r1");
    for (const action of ["sessions.start", "sessions.send"])
      await expect(
        handleAgentApp(
          source,
          "r2",
          action,
          {
            prompt: "Fix",
            notifyOnComplete: true,
            ...(action === "sessions.send" ? { sessionId: "other" } : {}),
          },
          host,
        ),
      ).rejects.toThrow("only available in a Mono");
    expect(host.start).toHaveBeenCalledTimes(1);
    expect(host.send).not.toHaveBeenCalled();
  });

  it("reports a habit run's follow-up only when asked, to the Mono's chat", async () => {
    const { source, host } = habitFixture();
    await handleAgentApp(
      source,
      "s1",
      "sessions.send",
      { sessionId: "other", prompt: "Go on", notifyOnComplete: true },
      host,
    );
    expect(host.send).toHaveBeenLastCalledWith(
      "other",
      "Go on",
      "app-run-s1",
      "mono-chat",
      "each",
    );
    const plain = await handleAgentApp(
      source,
      "s2",
      "sessions.send",
      { sessionId: "other", prompt: "Go on" },
      host,
    );
    expect(plain).not.toHaveProperty("notifyOnComplete");
    expect(host.send).toHaveBeenLastCalledWith("other", "Go on", "app-run-s2");
  });
});

describe("per-session completion delivery", () => {
  it("passes each as the fifth argument on start and send", async () => {
    const { source, host } = monoFixture();
    await handleAgentApp(
      source,
      "s",
      "sessions.start",
      { prompt: "Review", completionDelivery: "each" },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.anything(),
      "app-lead-s",
      undefined,
      "lead",
      "each",
    );
    await handleAgentApp(
      source,
      "split",
      "sessions.start",
      { prompt: "Review", completionDelivery: "each", placement: "right" },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.anything(),
      "app-lead-split",
      { direction: "right", besideSessionId: "lead" },
      "lead",
      "each",
    );
    await handleAgentApp(
      source,
      "m",
      "sessions.send",
      { sessionId: "other", prompt: "Fix", notifyOnComplete: true, completionDelivery: "each" },
      host,
    );
    expect(host.send).toHaveBeenLastCalledWith(
      "other",
      "Fix",
      "app-lead-m",
      "lead",
      "each",
    );
  });

  it.each([undefined, "group"])(
    "keeps the old argument lists when completionDelivery is %s",
    async (completionDelivery) => {
      const { source, host } = monoFixture();
      const extra = completionDelivery ? { completionDelivery } : {};
      await handleAgentApp(source, "s", "sessions.start", { prompt: "Review", ...extra }, host);
      expect(host.start).toHaveBeenLastCalledWith(
        expect.anything(),
        "app-lead-s",
        undefined,
        "lead",
      );
      await handleAgentApp(
        source,
        "m",
        "sessions.send",
        { sessionId: "other", prompt: "Fix", notifyOnComplete: true, ...extra },
        host,
      );
      expect(host.send).toHaveBeenLastCalledWith("other", "Fix", "app-lead-m", "lead");
    },
  );

  it.each(["sessions.start", "sessions.send"])(
    "rejects an invalid completionDelivery for %s before submission",
    async (action) => {
      const { source, host } = monoFixture();
      for (const completionDelivery of ["batch", 1, true, null])
        await expect(
          handleAgentApp(
            source,
            "bad",
            action,
            {
              prompt: "Review",
              completionDelivery,
              ...(action === "sessions.send" ? { sessionId: "other" } : {}),
            },
            host,
          ),
        ).rejects.toThrow('completionDelivery must be "group" or "each"');
      expect(host.start).not.toHaveBeenCalled();
      expect(host.send).not.toHaveBeenCalled();
    },
  );

  it("ignores a valid completionDelivery when nothing is monitored", async () => {
    const { source, host } = monoFixture();
    await handleAgentApp(
      source,
      "d",
      "sessions.start",
      { prompt: "Review", draft: true, completionDelivery: "each" },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ draft: true }),
      "app-lead-d",
    );
    await handleAgentApp(
      source,
      "n",
      "sessions.start",
      { prompt: "Review", notifyOnComplete: false, completionDelivery: "each" },
      host,
    );
    expect(host.start).toHaveBeenLastCalledWith(expect.anything(), "app-lead-n");
    await handleAgentApp(
      source,
      "sn",
      "sessions.send",
      { sessionId: "other", prompt: "Fix", completionDelivery: "each" },
      host,
    );
    expect(host.send).toHaveBeenLastCalledWith("other", "Fix", "app-lead-sn");
  });
});

describe("starting a session under another session", () => {
  const ids = ["app-lead-dev"];

  function parentFixture() {
    const fx = monoFixture();
    fx.launches.set("app-lead-dev", { monoId: "mono", parented: false });
    listSessions(fx.host, fx.source.cwd, ids);
    return fx;
  }

  it("nests the id under the parent and records the launch", async () => {
    const { source, host } = parentFixture();
    const result = await handleAgentApp(
      source,
      "rev",
      "sessions.start",
      { prompt: "Review the PR", parentSessionId: "app-lead-dev" },
      host,
    );
    expect(result).toMatchObject({
      id: "app-app-lead-dev-rev",
      notifyOnComplete: true,
    });
    expect(host.start).toHaveBeenCalledWith(
      expect.anything(),
      "app-app-lead-dev-rev",
      undefined,
      "lead",
    );
    expect(host.launches!.record).toHaveBeenCalledWith(
      "app-app-lead-dev-rev",
      "mono",
      true,
    );
  });

  it("records plain launches as unparented and ignores a missing registry", async () => {
    const { source, host } = parentFixture();
    await handleAgentApp(source, "dev2", "sessions.start", { prompt: "Fix" }, host);
    expect(host.launches!.record).toHaveBeenCalledWith("app-lead-dev2", "mono", false);
    delete host.launches;
    await handleAgentApp(source, "dev3", "sessions.start", { prompt: "Fix" }, host);
  });

  it("does not record a launch whose start failed", async () => {
    const { source, host } = parentFixture();
    host.start = vi.fn(async () => {
      throw new Error("boom");
    });
    await expect(
      handleAgentApp(source, "dev2", "sessions.start", { prompt: "Fix" }, host),
    ).rejects.toThrow("boom");
    expect(host.launches!.record).not.toHaveBeenCalled();
  });

  it("gives a retry with the same request ID the same nested ID", async () => {
    const { source, host } = parentFixture();
    const input = { prompt: "Review", parentSessionId: "app-lead-dev" };
    const first = await handleAgentApp(source, "rev", "sessions.start", input, host);
    const second = await handleAgentApp(source, "rev", "sessions.start", input, host);
    expect(second).toMatchObject({ id: (first as { id: string }).id });
  });

  it("lets a habit run parent a session under a launch it made earlier", async () => {
    const { source, host } = habitFixture();
    const launches = new Map<string, MonoLaunch>([
      ["app-gone-dev", { monoId: "mono", parented: false }],
    ]);
    host.launches = {
      record: vi.fn((s, m, p) => void launches.set(s, { monoId: m, parented: p })),
      get: (s) => launches.get(s),
    };
    listSessions(host, source.cwd, ["app-gone-dev"]);
    const result = await handleAgentApp(
      source,
      "rev",
      "sessions.start",
      { prompt: "Review", parentSessionId: "app-gone-dev" },
      host,
    );
    expect(result).toMatchObject({ id: "app-app-gone-dev-rev" });
    expect(host.launches.record).toHaveBeenCalledWith("app-app-gone-dev-rev", "mono", true);
  });

  it("rejects a caller that is not a Mono", async () => {
    const { source, host } = fixture();
    host.launches = {
      record: vi.fn(),
      get: () => ({ monoId: "mono", parented: false }),
    };
    listSessions(host, source.cwd, ["other"]);
    await expect(
      handleAgentApp(
        source,
        "rev",
        "sessions.start",
        { prompt: "Review", parentSessionId: "other" },
        host,
      ),
    ).rejects.toThrow("only available to a Mono");
    expect(host.start).not.toHaveBeenCalled();
  });

  it.each([
    ["a session the user started", "user-session"],
    ["the Mono's own chat", "lead"],
    ["an unknown session", "nope"],
    ["a session another Mono started", "app-other-dev"],
  ])("rejects %s as a parent", async (_name, parent) => {
    const { source, host, launches } = parentFixture();
    launches.set("app-other-dev", { monoId: "other-mono", parented: false });
    listSessions(host, source.cwd, [...ids, "user-session", "app-other-dev", "lead"]);
    await expect(
      handleAgentApp(
        source,
        "rev",
        "sessions.start",
        { prompt: "Review", parentSessionId: parent },
        host,
      ),
    ).rejects.toThrow("must be a session this Mono started");
    expect(host.start).not.toHaveBeenCalled();
    expect(host.launches!.record).not.toHaveBeenCalled();
  });

  it("rejects a parent that is not listed in the project", async () => {
    const { source, host } = parentFixture();
    listSessions(host, source.cwd, []);
    await expect(
      handleAgentApp(
        source,
        "rev",
        "sessions.start",
        { prompt: "Review", parentSessionId: "app-lead-dev" },
        host,
      ),
    ).rejects.toThrow("Session was not found in this project");
    expect(host.start).not.toHaveBeenCalled();
  });

  it("rejects a parent that was itself started under a parent", async () => {
    const { source, host, launches } = parentFixture();
    launches.set("app-app-lead-dev-rev", { monoId: "mono", parented: true });
    listSessions(host, source.cwd, [...ids, "app-app-lead-dev-rev"]);
    await expect(
      handleAgentApp(
        source,
        "deep",
        "sessions.start",
        { prompt: "Review", parentSessionId: "app-app-lead-dev-rev" },
        host,
      ),
    ).rejects.toThrow("child of a child");
    expect(host.start).not.toHaveBeenCalled();
  });

  it("rejects a malformed or oversized parent and an oversized resulting ID", async () => {
    const { source, host, launches } = parentFixture();
    for (const parentSessionId of [7, "", "x".repeat(257)])
      await expect(
        handleAgentApp(
          source,
          "rev",
          "sessions.start",
          { prompt: "Review", parentSessionId },
          host,
        ),
      ).rejects.toThrow("parentSessionId must be a non-empty string");
    const parent = "p".repeat(200);
    launches.set(parent, { monoId: "mono", parented: false });
    listSessions(host, source.cwd, [parent]);
    await expect(
      handleAgentApp(
        source,
        "r".repeat(100),
        "sessions.start",
        { prompt: "Review", parentSessionId: parent },
        host,
      ),
    ).rejects.toThrow("exceed 255 characters");
    expect(host.start).not.toHaveBeenCalled();
  });

  it("allows a resulting ID of 255 characters and rejects 256, the longest a directory name can be", async () => {
    const { source, host, launches } = parentFixture();
    const parent = "p".repeat(200);
    launches.set(parent, { monoId: "mono", parented: false });
    listSessions(host, source.cwd, [parent]);
    // "app-" + parent + "-" + request
    const fits = "r".repeat(255 - 4 - parent.length - 1);
    await expect(
      handleAgentApp(
        source,
        `${fits}x`,
        "sessions.start",
        { prompt: "Review", parentSessionId: parent },
        host,
      ),
    ).rejects.toThrow("exceed 255 characters");
    expect(host.start).not.toHaveBeenCalled();
    const result = (await handleAgentApp(
      source,
      fits,
      "sessions.start",
      { prompt: "Review", parentSessionId: parent },
      host,
    )) as { id: string };
    expect(result.id).toHaveLength(255);
    expect(host.start).toHaveBeenCalledOnce();
  });
});

describe("the chat that receives a caller's completion reports", () => {
  const monoOf = (id: string) =>
    id === "run" ? { sessionId: "mono-chat" } : { sessionId: undefined };

  it("is the Mono's own chat for a Mono, and the Mono's chat for its habit run", () => {
    const host = {
      isMono: (id: string) => id === "mono-chat",
      isHabitRun: (id: string) => id === "run",
      monoOf,
    };
    expect(completionChatId("mono-chat", host)).toBe("mono-chat");
    expect(completionChatId("run", host)).toBe("mono-chat");
  });

  it("is nothing for a regular session or a habit run whose Mono chat is unknown", () => {
    const host = {
      isMono: () => false,
      isHabitRun: (id: string) => id === "run" || id === "orphan",
      monoOf,
    };
    expect(completionChatId("plain", host)).toBeUndefined();
    expect(completionChatId("orphan", host)).toBeUndefined();
  });
});

describe("choosing a folder when starting a session", () => {
  const folder = (id: string, name: string, sessionIds: string[] = ["old"]) => ({
    id,
    name,
    sessionIds,
    collapsed: false,
  });

  it("files into an existing folder without renaming it", async () => {
    const { source, host } = monoFixture();
    saveSessionFolders(source.cwd, [folder("f1", "Ticket 1")]);
    const result = await handleAgentApp(
      source,
      "dev",
      "sessions.start",
      { prompt: "Fix", folderId: "f1", folderName: "Something else" },
      host,
    );
    expect(result).toMatchObject({ folderId: "f1", folderName: "Ticket 1" });
    expect(loadSessionFolders(source.cwd)).toEqual([
      expect.objectContaining({ id: "f1", name: "Ticket 1", sessionIds: ["old", "app-lead-dev"] }),
    ]);
  });

  it("creates a folder with the given name for an unknown folderId", async () => {
    const { source, host } = monoFixture();
    const result = await handleAgentApp(
      source,
      "dev",
      "sessions.start",
      { prompt: "Fix", folderId: "gone", folderName: "Ticket 2" },
      host,
    );
    expect(loadSessionFolders(source.cwd)).toEqual([
      expect.objectContaining({ name: "Ticket 2", sessionIds: ["app-lead-dev"] }),
    ]);
    expect(result).toMatchObject({
      folderId: loadSessionFolders(source.cwd)[0].id,
      folderName: "Ticket 2",
    });
  });

  it("rejects an unknown folderId alone before starting", async () => {
    const { source, host } = monoFixture();
    await expect(
      handleAgentApp(
        source,
        "dev",
        "sessions.start",
        { prompt: "Fix", folderId: "gone" },
        host,
      ),
    ).rejects.toThrow("Folder was not found in this project");
    expect(host.start).not.toHaveBeenCalled();
  });

  it("never merges launches that share a folderName", async () => {
    const { source, host } = monoFixture();
    for (const request of ["a", "b"])
      await handleAgentApp(
        source,
        request,
        "sessions.start",
        { prompt: "Fix", folderName: "Ticket 7 (2026-10-21)" },
        host,
      );
    const folders = loadSessionFolders(source.cwd);
    expect(folders).toHaveLength(2);
    expect(folders.map((entry) => entry.name)).toEqual([
      "Ticket 7 (2026-10-21)",
      "Ticket 7 (2026-10-21)",
    ]);
    expect(folders[0].id).not.toBe(folders[1].id);
  });

  it("keeps a retried launch in the folder it made", async () => {
    const { source, host } = monoFixture();
    const input = { prompt: "Fix", folderName: "Ticket 9" };
    const first = (await handleAgentApp(source, "dev", "sessions.start", input, host)) as {
      folderId: string;
    };
    saveSessionFolders(source.cwd, [
      ...loadSessionFolders(source.cwd).map((entry) => ({
        ...entry,
        sessionIds: [...entry.sessionIds, "app-lead-rev"],
      })),
    ]);
    const second = await handleAgentApp(source, "dev", "sessions.start", input, host);
    expect(second).toMatchObject({ folderId: first.folderId, folderName: "Ticket 9" });
    expect(loadSessionFolders(source.cwd)).toEqual([
      expect.objectContaining({ sessionIds: ["app-lead-dev", "app-lead-rev"] }),
    ]);
  });

  it.each([
    ["folderName over 100 characters", { folderName: "n".repeat(101) }],
    ["an empty folderName", { folderName: "" }],
    ["a blank folderName", { folderName: "   " }],
    ["a non-string folderName", { folderName: 5 }],
    ["a non-string folderId", { folderId: 5 }],
  ])("rejects %s before starting", async (_name, fields) => {
    const { source, host } = monoFixture();
    await expect(
      handleAgentApp(source, "dev", "sessions.start", { prompt: "Fix", ...fields }, host),
    ).rejects.toThrow("must be a non-empty string");
    expect(host.start).not.toHaveBeenCalled();
    expect(loadSessionFolders(source.cwd)).toEqual([]);
  });

  it("colors a new folder with the Mono's color, falling back to its folder color", async () => {
    const colored = monoFixture({ color: "#112233", folder: { name: "Jack", color: "#aabbcc" } });
    await handleAgentApp(colored.source, "a", "sessions.start", { prompt: "Fix", folderName: "T" }, colored.host);
    expect(loadSessionFolders(colored.source.cwd)[0]).toMatchObject({ customColor: "#112233" });
    localStorage.clear();
    const fallback = monoFixture({ folder: { name: "Jack", color: "#aabbcc" } });
    await handleAgentApp(fallback.source, "a", "sessions.start", { prompt: "Fix", folderName: "T" }, fallback.host);
    expect(loadSessionFolders(fallback.source.cwd)[0]).toMatchObject({ name: "T", customColor: "#aabbcc" });
  });

  it("creates an uncolored folder for a caller that is not a Mono", async () => {
    const { source, host } = fixture();
    const result = await handleAgentApp(
      source,
      "dev",
      "sessions.start",
      { prompt: "Fix", folderName: "Mine" },
      host,
    );
    const [created] = loadSessionFolders(source.cwd);
    expect(created).toMatchObject({ name: "Mine", sessionIds: ["app-lead-dev"] });
    expect(created.customColor).toBeUndefined();
    expect(result).toMatchObject({ folderId: created.id, folderName: "Mine" });
  });

  it("neither files nor creates a folder for a hidden launch, but still validates", async () => {
    const { source, host } = monoFixture({ showStartedSessionsInSidebar: false });
    const result = await handleAgentApp(
      source,
      "dev",
      "sessions.start",
      { prompt: "Fix", folderName: "Ticket" },
      host,
    );
    expect(loadSessionFolders(source.cwd)).toEqual([]);
    expect(result).not.toHaveProperty("folderId");
    await expect(
      handleAgentApp(
        source,
        "dev2",
        "sessions.start",
        { prompt: "Fix", folderId: "gone" },
        host,
      ),
    ).rejects.toThrow("Folder was not found in this project");
    expect(host.start).toHaveBeenCalledTimes(1);
  });

  it("keeps the Mono's default folder when no folder field is given", async () => {
    const { source, host } = monoFixture({ folder: { name: "Captain Jack", color: "#5b8def" } });
    const result = await handleAgentApp(source, "dev", "sessions.start", { prompt: "Fix" }, host);
    expect(loadSessionFolders(source.cwd)).toEqual([
      expect.objectContaining({ name: "Captain Jack", sessionIds: ["app-lead-dev"] }),
    ]);
    expect(result).not.toHaveProperty("folderId");
    expect(result).not.toHaveProperty("folderName");
  });

  it("files a nested reviewer into the folder of its dev", async () => {
    const { source, host, launches } = monoFixture();
    saveSessionFolders(source.cwd, [folder("f1", "Ticket 1", ["app-lead-dev"])]);
    launches.set("app-lead-dev", { monoId: "mono", parented: false });
    listSessions(host, source.cwd, ["app-lead-dev"]);
    await handleAgentApp(
      source,
      "rev",
      "sessions.start",
      { prompt: "Review", parentSessionId: "app-lead-dev", folderId: "f1" },
      host,
    );
    expect(loadSessionFolders(source.cwd)).toEqual([
      expect.objectContaining({ sessionIds: ["app-lead-dev", "app-app-lead-dev-rev"] }),
    ]);
  });
});
