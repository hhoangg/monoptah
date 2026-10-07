// @vitest-environment happy-dom
import { invoke } from "@tauri-apps/api/core";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearInboxCache, type InboxItem } from "../model/githubTasks";
import { InboxDetail } from "./InboxView";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

// `identifier` is display text and must never reach a ClickUp command.
const item: InboxItem = {
  provider: "clickup",
  kind: "clickup",
  id: "86abc12",
  identifier: "ENG-42",
  number: 42,
  title: "Fix the export",
  url: "https://app.clickup.com/t/86abc12",
  state: "in progress",
  stateType: "indeterminate",
  updatedAt: "2026-10-06T12:00:00Z",
  labels: [],
  assignees: [],
  draft: false,
  repo: "Engineering",
  teamId: "901",
  teamName: "Engineering",
  projectName: "Sprint 4",
  projectPath: "",
};

const comment = {
  id: "c1",
  kind: "comment",
  author: "bob",
  body: "Reproduced on staging",
  createdAt: "2026-10-06T11:00:00Z",
  url: "https://app.clickup.com/t/86abc12",
  state: "",
  path: "",
  line: null,
  resolved: false,
  threadId: "",
  replies: [],
};

let root: Root;
let container: HTMLDivElement;
let thread = { comments: [comment] };

const calls = (name: string) =>
  vi.mocked(invoke).mock.calls.filter(([command]) => command === name);

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  clearInboxCache();
  thread = { comments: [comment] };
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "clickup_issue_details")
      return { body: "Export drops the last row", author: "ada" };
    if (command === "clickup_issue_thread")
      return {
        ...thread,
        truncated: false,
        reviewDecision: "",
        baseRefName: "",
        headRefName: "",
      };
    if (command === "clickup_issue_comment") {
      thread = {
        comments: [
          ...thread.comments,
          {
            ...comment,
            id: "c2",
            author: "ada",
            body: (args as { body: string }).body,
          },
        ],
      };
      return item.url;
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

async function render(props: Partial<Parameters<typeof InboxDetail>[0]> = {}) {
  await act(async () =>
    root.render(
      createElement(InboxDetail, {
        item,
        cwd: "/tmp/web",
        projects: [
          {
            path: "/tmp/web",
            name: "web",
            logoPath: null,
            mascotName: null,
            mascotColor: "",
          },
        ],
        revision: 0,
        relatedSessions: [],
        onDiscuss: () => {},
        onStart: () => {},
        ...props,
      }),
    ),
  );
}

function button(label: string) {
  return [...container.querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes(label),
  )!;
}

it("shows the description and comments, loaded by the raw task id", async () => {
  await render();
  expect(container.textContent).toContain("Task");
  expect(container.textContent).toContain("ENG-42");
  expect(container.textContent).toContain("Engineering / Sprint 4");
  expect(container.textContent).toContain("Open in ClickUp");
  expect(container.textContent).toContain("Export drops the last row");
  expect(container.textContent).toContain("Reproduced on staging");
  expect(container.textContent).toContain("1 comment");
  expect(calls("clickup_issue_details")).toEqual([
    ["clickup_issue_details", { taskId: "86abc12" }],
  ]);
  expect(calls("clickup_issue_thread")).toEqual([
    ["clickup_issue_thread", { taskId: "86abc12" }],
  ]);
});

it("posts a comment with the raw task id and shows the refreshed thread", async () => {
  await render();
  const field = container.querySelector<HTMLTextAreaElement>("textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )!.set!.call(field, "  On it  ");
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => button("Comment").click());
  expect(calls("clickup_issue_comment")).toEqual([
    ["clickup_issue_comment", { taskId: "86abc12", body: "On it" }],
  ]);
  expect(calls("clickup_issue_thread")).toHaveLength(2);
  expect(container.textContent).toContain("2 comments");
  expect(container.textContent).toContain("On it");
});

it("hands the loaded description to Send to agent and keeps Ask available", async () => {
  const onStart = vi.fn();
  const onDiscuss = vi.fn();
  await render({ onStart, onDiscuss });
  await act(async () => button("Send to agent").click());
  expect(onStart).toHaveBeenCalledWith(
    { ...item, projectPath: "/tmp/web" },
    "Export drops the last row",
  );
  await act(async () => button("Ask").click());
  expect(onDiscuss).toHaveBeenCalledTimes(1);
});
