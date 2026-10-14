// @vitest-environment happy-dom
import { invoke } from "@tauri-apps/api/core";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { clearInboxCache, inboxItemKey } from "../model/githubTasks";
import type { InboxItem } from "../model/githubTasks";
import {
  getRememberedInboxView,
  resetRememberedInboxViews,
  setRememberedInboxView,
} from "../model/inboxViewMemory";
import { CLICKUP_CHANGE_EVENT } from "../model/clickup";
import type { LinkedWorkItem } from "../../sessions/model/session";
import { InboxView } from "./InboxView";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
  }),
}));

function task(id: string, title: string) {
  return {
    provider: "clickup",
    kind: "clickup",
    id,
    identifier: `#${id}`,
    number: 0,
    title,
    url: `https://app.clickup.com/t/${id}`,
    state: "open",
    stateType: "new",
    updatedAt: "2026-10-06T12:00:00.000Z",
    labels: [],
    assignees: [],
    draft: false,
    repo: "Engineering",
    teamId: "901",
    teamName: "Engineering",
    listName: "Sprint 4",
  };
}

function linearIssue(id: string, title: string) {
  return {
    provider: "linear",
    kind: "linear",
    id,
    identifier: `ENG-${id}`,
    number: Number(id),
    title,
    url: `https://linear.app/acme/issue/ENG-${id}`,
    state: "Todo",
    stateType: "unstarted",
    updatedAt: "2026-10-06T12:00:00.000Z",
    labels: [],
    assignees: [],
    draft: false,
    repo: "Engineering",
    teamId: "t1",
    teamName: "Engineering",
    projectId: "",
    projectName: "",
    projectPath: "",
  };
}

let clickupConnected: boolean;
let clickupIssues: () => Promise<unknown[]>;
let root: Root;
let container: HTMLDivElement;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  resetRememberedInboxViews();
  localStorage.setItem("monocode.inboxSource", "clickup");
  localStorage.setItem(
    "monocode.inboxConnections",
    JSON.stringify({
      github: true,
      linear: true,
      jira: false,
      clickup: true,
      gitlab: false,
      azuredevops: false,
    }),
  );
  clearInboxCache();
  vi.mocked(invoke).mockReset();
  clickupConnected = true;
  clickupIssues = async () => [
    task("a1", "Fix export"),
    task("b2", "Logo refresh"),
    task("c3", "Logo colours"),
  ];
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "clickup_status")
      return {
        connected: clickupConnected,
        teamId: "1",
        teamName: "Acme",
        username: "ada",
      };
    if (command === "linear_status" || command === "git_github_status")
      return { connected: true };
    if (command.endsWith("_status"))
      return { connected: false, site: "", email: "" };
    if (command === "clickup_list_spaces") return [];
    if (command === "clickup_list_issues") return clickupIssues();
    if (command === "linear_list_teams") return [];
    if (command === "linear_list_issues")
      return [
        linearIssue("1", "Linear one"),
        linearIssue("2", "Linear two"),
        linearIssue("3", "Linear three"),
      ];
    if (command === "linear_issue_details") return { body: "", author: "" };
    if (command === "linear_issue_thread")
      return {
        comments: [],
        truncated: false,
        reviewDecision: "",
        baseRefName: "",
        headRefName: "",
      };
    if (command === "git_github_work_item")
      return {
        kind: "pr",
        number: 7,
        title: "Targeted PR",
        url: "https://github.com/acme/web/pull/7",
        state: "open",
        updatedAt: "2026-10-06T12:00:00.000Z",
        labels: [],
        assignees: [],
        draft: false,
        repo: "acme/web",
      };
    if (command === "clickup_issue_details") return { body: "", author: "" };
    if (command === "clickup_issue_thread")
      return {
        comments: [],
        truncated: false,
        reviewDecision: "",
        baseRefName: "",
        headRefName: "",
      };
    throw new Error(`Unexpected command: ${command}`);
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  localStorage.clear();
  resetRememberedInboxViews();
  vi.unstubAllGlobals();
});

async function mount(target?: LinkedWorkItem) {
  await act(async () =>
    root.render(
      createElement(InboxView, {
        onAsk: async () => "",
        onAskRestart: async () => "",
        onAskMount: () => {},
        cwd: "",
        recents: [],
        target,
        onOpenIntegrations: () => {},
      }),
    ),
  );
}

async function leaveAndReturn(target?: LinkedWorkItem) {
  await act(async () => root.unmount());
  root = createRoot(container);
  await mount(target);
}

const tab = (label: string) =>
  [...container.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find(
    (entry) => entry.textContent === label,
  )!;

const rowButton = (title: string) =>
  [...container.querySelectorAll<HTMLButtonElement>("li button")].find(
    (button) => button.title === title,
  )!;

const searchBox = () =>
  container.querySelector<HTMLInputElement>(
    'input[aria-label="Filter inbox"]',
  )!;

const activeCard = () =>
  container.querySelector<HTMLButtonElement>('button[aria-current="true"]');

async function search(text: string) {
  const input = searchBox();
  const setValue = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  await act(async () => {
    setValue.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

it("selects the first row on a fresh start", async () => {
  await mount();
  expect(activeCard()?.title).toBe("Fix export");
  expect(searchBox().value).toBe("");
});

it("restores the selected row after leaving the Inbox and coming back", async () => {
  await mount();
  const second = [
    ...container.querySelectorAll<HTMLButtonElement>("li button"),
  ].find((button) => button.title === "Logo colours")!;
  await act(async () => second.click());
  expect(activeCard()?.title).toBe("Logo colours");

  await leaveAndReturn();
  expect(activeCard()?.title).toBe("Logo colours");
});

it("restores the search text and the row picked inside it", async () => {
  await mount();
  await search("logo");
  expect(container.textContent).not.toContain("Fix export");
  const second = [
    ...container.querySelectorAll<HTMLButtonElement>("li button"),
  ].find((button) => button.title === "Logo colours")!;
  await act(async () => second.click());

  await leaveAndReturn();
  expect(searchBox().value).toBe("logo");
  expect(container.textContent).not.toContain("Fix export");
  expect(activeCard()?.title).toBe("Logo colours");
});

it("does not carry a remembered view across an app restart", async () => {
  await mount();
  await search("logo");
  await act(async () => root.unmount());
  resetRememberedInboxViews();
  root = createRoot(container);
  await mount();
  expect(searchBox().value).toBe("");
  expect(container.textContent).toContain("Fix export");
});

it("saves and restores search and selection for each source tab", async () => {
  await mount();
  await search("logo");
  await act(async () => rowButton("Logo colours").click());

  await act(async () => tab("Linear").click());
  expect(searchBox().value).toBe("");
  expect(activeCard()?.title).toBe("Linear three");
  await search("linear");
  await act(async () => rowButton("Linear one").click());

  await act(async () => tab("ClickUp").click());
  expect(searchBox().value).toBe("logo");
  expect(activeCard()?.title).toBe("Logo colours");

  await act(async () => tab("Linear").click());
  expect(searchBox().value).toBe("linear");
  expect(activeCard()?.title).toBe("Linear one");

  // Both tabs come back after leaving too.
  await leaveAndReturn();
  expect(searchBox().value).toBe("linear");
  expect(activeCard()?.title).toBe("Linear one");
  await act(async () => tab("ClickUp").click());
  expect(searchBox().value).toBe("logo");
});

it("keeps the old tab's view when disconnecting moves you to another tab", async () => {
  setRememberedInboxView("github", {
    search: "gh",
    selectedKey: null,
    scrollTop: 0,
    listLimit: null,
  });
  await mount();
  await search("logo");

  clickupConnected = false;
  await act(async () => {
    window.dispatchEvent(new Event(CLICKUP_CHANGE_EVENT));
  });
  expect(tab("GitHub").getAttribute("aria-selected")).toBe("true");
  expect(searchBox().value).toBe("gh");

  await act(async () => root.unmount());
  expect(getRememberedInboxView("clickup").search).toBe("logo");
  expect(getRememberedInboxView("github").search).toBe("gh");
});

it("lets a linked target win without overwriting the remembered GitHub view", async () => {
  const remembered = {
    search: "keepme",
    selectedKey: "github:acme/web:issue:1",
    scrollTop: 0,
    listLimit: null,
  };
  setRememberedInboxView("github", remembered);
  const target: LinkedWorkItem = {
    kind: "pr",
    repo: "acme/web",
    number: 7,
    url: "https://github.com/acme/web/pull/7",
  };

  await mount(target);
  expect(searchBox().value).toBe("");
  expect(activeCard()?.title).toBe("Targeted PR");

  await act(async () => root.unmount());
  expect(getRememberedInboxView("github")).toEqual(remembered);
});

it("keeps the remembered selection while the first load is still in flight", async () => {
  setRememberedInboxView("clickup", {
    search: "",
    selectedKey: inboxItemKey(task("c3", "Logo colours") as InboxItem),
    scrollTop: 0,
    listLimit: null,
  });
  let finish: (items: unknown[]) => void = () => {};
  clickupIssues = () =>
    new Promise<unknown[]>((resolve) => {
      finish = resolve;
    });

  await mount();
  expect(container.querySelector("li button")).toBeNull();
  await act(async () =>
    finish([
      task("a1", "Fix export"),
      task("b2", "Logo refresh"),
      task("c3", "Logo colours"),
    ]),
  );
  expect(activeCard()?.title).toBe("Logo colours");
});

it("restores a source's scroll offset even when both lists are the same length", async () => {
  setRememberedInboxView("linear", {
    search: "",
    selectedKey: null,
    scrollTop: 50,
    listLimit: null,
  });
  await mount();
  const scroller = container.querySelector<HTMLElement>(".overflow-y-auto")!;
  // happy-dom does not fire scroll for a programmatic scrollTop, so fire it.
  await act(async () => {
    scroller.scrollTop = 80;
    scroller.dispatchEvent(new Event("scroll"));
  });
  expect(container.querySelectorAll("li button")).toHaveLength(3);

  await act(async () => tab("Linear").click());
  expect(container.querySelectorAll("li button")).toHaveLength(3);
  expect(scroller.scrollTop).toBe(50);

  // Going back restores the offset the first tab was left at.
  await act(async () => tab("ClickUp").click());
  expect(scroller.scrollTop).toBe(80);
});
