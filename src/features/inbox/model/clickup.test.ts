// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  CLICKUP_CHANGE_EVENT,
  clearClickUpCache,
  clickupIssueComment,
  clickupIssueDetails,
  clickupIssueThread,
  clickupIssuesAssignedTo,
  clickupSpaceIdsForFetch,
  disconnectClickUp,
  loadHiddenClickUpSpaceIds,
  peekClickUpIssueDetails,
  peekClickUpIssueThread,
  reconcileHiddenClickUpSpaceIds,
  saveClickUpConfig,
  saveHiddenClickUpSpaceIds,
  type ClickUpIssue,
} from "./clickup";
import {
  clearInboxCache,
  inboxIdentityKey,
  inboxItemStatus,
  inboxStartDraft,
  listInboxItems,
} from "./githubTasks";
import { inboxTrackerDescription } from "./inboxContext";
import {
  clearPendingInboxSelfActivity,
  consumeInboxSelfActivity,
} from "./inboxSelfActivity";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const issue: ClickUpIssue = {
  provider: "clickup",
  kind: "clickup",
  id: "86abc12",
  identifier: "ENG-42",
  number: 42,
  title: "Fix auth",
  url: "https://app.clickup.com/t/86abc12",
  state: "in progress",
  stateType: "indeterminate",
  updatedAt: "2026-09-23T10:00:00.000Z",
  labels: [],
  assignees: [{ login: "ada" }],
  draft: false,
  repo: "Engineering",
  teamId: "1001",
  teamName: "Engineering",
  projectPath: "Sprint 12",
};
const spaces = [
  { id: "1001", name: "Engineering" },
  { id: "1002", name: "Operations" },
];
const query = { assignedToMe: false, state: "open", search: "" } as const;

function listIssueCalls() {
  return vi
    .mocked(invoke)
    .mock.calls.filter(([command]) => command === "clickup_list_issues");
}

beforeEach(() => {
  clearInboxCache();
  clearPendingInboxSelfActivity();
  localStorage.clear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "clickup_status")
      return { connected: true, teamId: "9", teamName: "Acme", username: "Ada" };
    if (command.endsWith("_status")) return { connected: false };
    if (command === "clickup_list_spaces") return spaces;
    if (command === "clickup_list_issues") return [issue];
    if (command === "clickup_issue_details")
      return { body: "Reproduction steps", author: "Ada" };
    if (command === "clickup_issue_thread")
      return { comments: [], truncated: false };
    if (command === "clickup_issue_comment") return issue.url;
    if (command === "clickup_set_config")
      return { connected: false, teamId: "", teamName: "", username: "" };
    throw new Error(`Unexpected command: ${command}`);
  });
});

describe("ClickUp hidden spaces", () => {
  it("reconciles hidden ids against the live space list", () => {
    expect(reconcileHiddenClickUpSpaceIds(spaces, ["1002", "gone"])).toEqual([
      "1002",
    ]);
    expect(reconcileHiddenClickUpSpaceIds([], ["1001"])).toEqual([]);
  });

  it("builds the space filter only from live ids", () => {
    expect(clickupSpaceIdsForFetch(spaces, [])).toBeNull();
    expect(clickupSpaceIdsForFetch(spaces, ["1002"])).toEqual(["1001"]);
    expect(clickupSpaceIdsForFetch(spaces, ["1001", "1002"])).toEqual([]);
    // A stale id alone hides nothing, so the request is not narrowed.
    expect(clickupSpaceIdsForFetch(spaces, ["deleted"])).toBeNull();
    expect(clickupSpaceIdsForFetch(spaces, ["1002", "deleted"])).toEqual([
      "1001",
    ]);
  });

  it("persists, reloads and announces hidden spaces", () => {
    const onChange = vi.fn();
    window.addEventListener(CLICKUP_CHANGE_EVENT, onChange);
    saveHiddenClickUpSpaceIds(["1001"]);
    expect(localStorage.getItem("monocode.clickupHiddenSpaces")).toBe(
      '["1001"]',
    );
    expect(loadHiddenClickUpSpaceIds()).toEqual(["1001"]);
    expect(onChange).toHaveBeenCalledTimes(1);
    saveHiddenClickUpSpaceIds([]);
    expect(loadHiddenClickUpSpaceIds()).toEqual([]);
    expect(onChange).toHaveBeenCalledTimes(2);
    window.removeEventListener(CLICKUP_CHANGE_EVENT, onChange);
    expect(CLICKUP_CHANGE_EVENT).toBe("monocode:clickup-change");
  });

  it("ignores malformed stored values", () => {
    localStorage.setItem("monocode.clickupHiddenSpaces", "not json");
    expect(loadHiddenClickUpSpaceIds()).toEqual([]);
    localStorage.setItem("monocode.clickupHiddenSpaces", '{"a":1}');
    expect(loadHiddenClickUpSpaceIds()).toEqual([]);
    localStorage.setItem("monocode.clickupHiddenSpaces", '["1001",7,""]');
    expect(loadHiddenClickUpSpaceIds()).toEqual(["1001"]);
  });
});

describe("ClickUp assignee filter", () => {
  it("matches the cached username case-insensitively", () => {
    const other = { ...issue, id: "2", assignees: [{ login: "bob" }] };
    const unassigned = { ...issue, id: "3", assignees: [] };
    expect(
      clickupIssuesAssignedTo([issue, other, unassigned], " ADA ").map(
        (entry) => entry.id,
      ),
    ).toEqual(["86abc12"]);
  });

  it("fails closed when the username is unknown", () => {
    expect(clickupIssuesAssignedTo([issue], "")).toEqual([]);
    expect(clickupIssuesAssignedTo([issue], "   ")).toEqual([]);
  });
});

describe("ClickUp inbox", () => {
  it("loads workspace-wide tasks without a local repository", async () => {
    const result = await listInboxItems([], query);
    expect(result.errors).toEqual({});
    expect(result.items).toHaveLength(1);
    expect(result.items[0]).toMatchObject({
      provider: "clickup",
      kind: "clickup",
      id: "86abc12",
      identifier: "ENG-42",
      teamId: "1001",
      teamName: "Engineering",
    });
    expect(listIssueCalls()).toEqual([
      [
        "clickup_list_issues",
        { spaceIds: [], includeClosed: false, limit: undefined },
      ],
    ]);
    expect(invoke).not.toHaveBeenCalledWith("clickup_list_spaces");
    expect(inboxItemStatus(result.items[0]!)).toBe("Open");
    expect(inboxItemStatus({ ...result.items[0]!, stateType: "done" })).toBe(
      "Closed",
    );
  });

  it("keeps the list name out of projectPath so it never becomes a cwd", async () => {
    const { items } = await listInboxItems([], query);
    expect(items[0]!.projectPath).toBe("");
    expect(items[0]!.projectName).toBe("Sprint 12");
  });

  it("includes closed tasks when every state is requested", async () => {
    await listInboxItems([], { ...query, state: "all" });
    expect(listIssueCalls()[0]![1]).toMatchObject({ includeClosed: true });
    expect(
      (listIssueCalls()[0]![1] as { limit?: number }).limit,
    ).toBeGreaterThan(0);
  });

  it("sends only live space ids and drops hidden ids for deleted spaces", async () => {
    await listInboxItems([], { ...query, clickupHiddenSpaceIds: ["1002"] });
    expect(listIssueCalls()[0]![1]).toMatchObject({ spaceIds: ["1001"] });

    vi.mocked(invoke).mockClear();
    clearInboxCache();
    // 1002 still exists, "gone" does not. The stale id must not be forwarded.
    await listInboxItems([], {
      ...query,
      clickupHiddenSpaceIds: ["1002", "gone"],
    });
    expect(listIssueCalls()[0]![1]).toMatchObject({ spaceIds: ["1001"] });

    vi.mocked(invoke).mockClear();
    clearInboxCache();
    // Only a stale id is stored: nothing real is hidden, so no narrowing.
    await listInboxItems([], { ...query, clickupHiddenSpaceIds: ["gone"] });
    expect(listIssueCalls()[0]![1]).toMatchObject({ spaceIds: [] });
  });

  it("reads the hidden spaces preference when the query does not carry one", async () => {
    saveHiddenClickUpSpaceIds(["1001", "gone"]);
    await listInboxItems([], query);
    expect(listIssueCalls()[0]![1]).toMatchObject({ spaceIds: ["1002"] });
  });

  it("skips the fetch and hides tasks when every space is hidden", async () => {
    expect(
      (
        await listInboxItems([], {
          ...query,
          clickupHiddenSpaceIds: ["1001", "1002"],
        })
      ).items,
    ).toEqual([]);
    expect(listIssueCalls()).toEqual([]);
  });

  it("filters hidden spaces out of the result as well", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "clickup_status") return { connected: true, username: "" };
      if (command.endsWith("_status")) return { connected: false };
      if (command === "clickup_list_spaces") return spaces;
      if (command === "clickup_list_issues")
        return [issue, { ...issue, id: "2", teamId: "1002" }];
      throw new Error(`Unexpected command: ${command}`);
    });
    const { items } = await listInboxItems([], {
      ...query,
      clickupHiddenSpaceIds: ["1002"],
    });
    expect(items.map((item) => item.id)).toEqual(["86abc12"]);
  });

  it("narrows to the connected user's tasks for assigned-to-me", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "clickup_status")
        return { connected: true, username: "ada" };
      if (command.endsWith("_status")) return { connected: false };
      if (command === "clickup_list_issues")
        return [
          issue,
          { ...issue, id: "2", assignees: [{ login: "bob" }] },
        ];
      throw new Error(`Unexpected command: ${command}`);
    });
    const { items } = await listInboxItems([], {
      ...query,
      assignedToMe: true,
    });
    expect(items.map((item) => item.id)).toEqual(["86abc12"]);
  });

  it("shows nothing for assigned-to-me when the cached username is empty", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "clickup_status") return { connected: true, username: "" };
      if (command.endsWith("_status")) return { connected: false };
      if (command === "clickup_list_issues") return [issue];
      throw new Error(`Unexpected command: ${command}`);
    });
    expect(
      (await listInboxItems([], { ...query, assignedToMe: true })).items,
    ).toEqual([]);
    clearInboxCache();
    expect((await listInboxItems([], query)).items).toHaveLength(1);
  });

  it("asks the backend for its maximum window when filtering to my tasks", async () => {
    await listInboxItems([], { ...query, assignedToMe: true });
    expect(listIssueCalls()[0]![1]).toMatchObject({
      includeClosed: false,
      limit: 100,
    });
    vi.mocked(invoke).mockClear();
    clearInboxCache();
    // Without the filter the backend default is kept.
    await listInboxItems([], query);
    expect(
      (listIssueCalls()[0]![1] as { limit?: number }).limit,
    ).toBeUndefined();
  });

  it("keeps GitHub items when ClickUp settings cannot be read", async () => {
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "clickup_status")
        throw new Error("ClickUp settings are invalid");
      if (command === "git_github_repositories") return ["acme/web"];
      if (command === "git_github_work_items") {
        return (args as { kind: string }).kind === "issue"
          ? [{ ...issue, kind: "issue", repo: "acme/web" }]
          : [];
      }
      return original(command, args);
    });
    const result = await listInboxItems([{ path: "/repo" }], query);
    expect(result.items).toHaveLength(1);
    expect(result.items[0]!.provider).toBe("github");
    expect(result.errors).toEqual({ clickup: "ClickUp settings are invalid" });
  });

  it("does not collide the cache across hidden space sets", async () => {
    await listInboxItems([], query);
    await listInboxItems([], { ...query, clickupHiddenSpaceIds: ["1002"] });
    expect(listIssueCalls()).toHaveLength(2);
  });

  it("keys identity on the raw id, not the display identifier", () => {
    expect(inboxIdentityKey(issue)).toBe("86abc12");
    expect(inboxIdentityKey({ ...issue, identifier: "ENG-43" })).toBe(
      "86abc12",
    );
    expect(inboxIdentityKey({ ...issue, id: "", identifier: "#x1" })).toBe("#x1");
  });

  it("starts sessions with a ClickUp task prompt", () => {
    const draft = inboxStartDraft(issue, "Steps");
    expect(draft).toContain("Work on this ClickUp task:");
    expect(draft).toContain("ENG-42 Fix auth");
    expect(draft).toContain("Steps");
  });
});

describe("ClickUp raw task ids", () => {
  it("loads the description with the raw id even when a custom id is shown", async () => {
    const description = await inboxTrackerDescription(issue);
    expect(invoke).toHaveBeenCalledWith("clickup_issue_details", {
      taskId: "86abc12",
    });
    expect(invoke).not.toHaveBeenCalledWith("clickup_issue_details", {
      taskId: "ENG-42",
    });
    expect(inboxStartDraft(issue, description)).toContain("Reproduction steps");
    vi.mocked(invoke).mockClear();
    expect(await inboxTrackerDescription(issue, "Provided")).toBe("Provided");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("refuses to fall back to the display identifier", async () => {
    await expect(inboxTrackerDescription({ ...issue, id: "" })).rejects.toThrow(
      "Missing ClickUp task",
    );
    await expect(clickupIssueDetails(" ")).rejects.toThrow(
      "Missing ClickUp task",
    );
    await expect(clickupIssueThread("")).rejects.toThrow("Missing ClickUp task");
    await expect(clickupIssueComment({ id: "" }, "hi")).rejects.toThrow(
      "Missing ClickUp task",
    );
    expect(invoke).not.toHaveBeenCalled();
  });

  it("shares an in-flight thread request and keeps the result by raw id", async () => {
    const first = clickupIssueThread(issue.id);
    const second = clickupIssueThread(issue.id);
    await Promise.all([first, second]);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(peekClickUpIssueThread(issue.id)).not.toBeNull();
    await clickupIssueThread(issue.id, { force: true });
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke).toHaveBeenLastCalledWith("clickup_issue_thread", {
      taskId: "86abc12",
    });
  });

  it("invalidates comments and suppresses notifications for the author's own comment", async () => {
    await clickupIssueThread(issue.id);
    expect(peekClickUpIssueThread(issue.id)).not.toBeNull();
    await clickupIssueComment({ id: issue.id }, "  Fixed\n\nPlease check  ");
    expect(invoke).toHaveBeenCalledWith("clickup_issue_comment", {
      taskId: "86abc12",
      body: "Fixed\n\nPlease check",
    });
    expect(peekClickUpIssueThread(issue.id)).toBeNull();
    expect(consumeInboxSelfActivity(issue)).toBe(true);
    expect(consumeInboxSelfActivity(issue)).toBe(false);
  });
});

describe("ClickUp connection", () => {
  it("clears credentials and cached descriptions on disconnect and reconnect", async () => {
    await clickupIssueDetails(issue.id);
    expect(peekClickUpIssueDetails(issue.id)).not.toBeNull();
    await disconnectClickUp();
    expect(invoke).toHaveBeenCalledWith("clickup_set_config", {
      token: "",
      teamId: null,
    });
    expect(peekClickUpIssueDetails(issue.id)).toBeNull();
    await clickupIssueDetails(issue.id);
    await saveClickUpConfig({ token: " pk_1 ", teamId: " 9 " });
    expect(invoke).toHaveBeenCalledWith("clickup_set_config", {
      token: "pk_1",
      teamId: "9",
    });
    expect(peekClickUpIssueDetails(issue.id)).toBeNull();
    await saveClickUpConfig({ token: "pk_1" });
    expect(invoke).toHaveBeenLastCalledWith("clickup_set_config", {
      token: "pk_1",
      teamId: null,
    });
  });

  it("does not restore a previous account's cache when a request finishes late", async () => {
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const pending = clickupIssueDetails(issue.id);
    clearClickUpCache();
    resolve({ body: "Old account", author: "Ada" });
    await pending;
    expect(peekClickUpIssueDetails(issue.id)).toBeNull();
  });
});
