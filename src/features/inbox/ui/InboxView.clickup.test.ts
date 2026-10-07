// @vitest-environment happy-dom
import { invoke } from "@tauri-apps/api/core";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  clearInboxCache,
  inboxListCacheKey,
  listInboxItems,
  peekInboxList,
} from "../model/githubTasks";
import {
  loadHiddenClickUpSpaceIds,
  saveHiddenClickUpSpaceIds,
} from "../model/clickup";
import { useInboxActivity } from "../hooks/useInboxUnseen";
import { InboxView } from "./InboxView";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// Spies that call through, so the real view and hook run and only record the
// project list and query they hand to the inbox cache.
vi.mock("../model/githubTasks", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../model/githubTasks")>();
  return {
    ...actual,
    listInboxItems: vi.fn(actual.listInboxItems),
    peekInboxList: vi.fn(actual.peekInboxList),
  };
});
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => false,
    onResized: async () => () => {},
  }),
}));

const SPACES = [
  { id: "901", name: "Engineering" },
  { id: "902", name: "Design" },
];

function task(id: string, title: string, spaceId: string) {
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
    repo: spaceId === "901" ? "Engineering" : "Design",
    teamId: spaceId,
    teamName: spaceId === "901" ? "Engineering" : "Design",
    listName: "Sprint 4",
  };
}

let root: Root;
let container: HTMLDivElement;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  localStorage.setItem("monocode.inboxSource", "clickup");
  localStorage.setItem(
    "monocode.inboxConnections",
    JSON.stringify({
      github: false,
      linear: false,
      jira: false,
      clickup: true,
      gitlab: false,
      azuredevops: false,
    }),
  );
  clearInboxCache();
  vi.mocked(listInboxItems).mockClear();
  vi.mocked(peekInboxList).mockClear();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "clickup_status")
      return {
        connected: true,
        teamId: "1",
        teamName: "Acme",
        username: "ada",
      };
    if (command.endsWith("_status"))
      return { connected: false, site: "", email: "" };
    if (command === "clickup_list_spaces") return SPACES;
    if (command === "clickup_list_issues") {
      const { spaceIds } = args as { spaceIds: string[] };
      return [
        task("a1", "Fix export", "901"),
        task("b2", "Logo", "902"),
      ].filter(
        (entry) => spaceIds.length === 0 || spaceIds.includes(entry.teamId),
      );
    }
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
  vi.unstubAllGlobals();
});

async function mount() {
  await act(async () =>
    root.render(
      createElement(InboxView, {
        onAsk: async () => "",
        onAskRestart: async () => "",
        onAskMount: () => {},
        cwd: "",
        recents: [],
        onOpenIntegrations: () => {},
      }),
    ),
  );
}

const issueCalls = () =>
  vi.mocked(invoke).mock.calls.filter(([c]) => c === "clickup_list_issues");

it("lists ClickUp tasks and fetches only the spaces that are not hidden", async () => {
  saveHiddenClickUpSpaceIds(["902"]);
  await mount();
  expect(issueCalls().at(-1)?.[1]).toMatchObject({ spaceIds: ["901"] });
  expect(container.textContent).toContain("Fix export");
  expect(container.textContent).not.toContain("Logo");
  // List name and space both show on the card.
  expect(container.textContent).toContain("Engineering / Sprint 4");
});

it("builds one list cache key in the view, its rail peek and the unseen hook", async () => {
  saveHiddenClickUpSpaceIds(["902"]);
  const cwd = "/tmp/app";
  const keyOf = (call: readonly unknown[]) =>
    inboxListCacheKey(
      call[0] as Parameters<typeof inboxListCacheKey>[0],
      call[1] as Parameters<typeof inboxListCacheKey>[1],
    );

  await act(async () =>
    root.render(
      createElement(InboxView, {
        onAsk: async () => "",
        onAskRestart: async () => "",
        onAskMount: () => {},
        cwd,
        recents: [],
        onOpenIntegrations: () => {},
      }),
    ),
  );
  // Every peek in the view (the rail peek in its initial state and the list
  // effect) and every fetch it starts.
  const viewKeys = [
    ...vi.mocked(peekInboxList).mock.calls,
    ...vi.mocked(listInboxItems).mock.calls,
  ].map(keyOf);
  expect(vi.mocked(peekInboxList).mock.calls.length).toBeGreaterThan(1);
  expect(vi.mocked(listInboxItems)).toHaveBeenCalled();
  await act(async () => root.unmount());

  vi.mocked(listInboxItems).mockClear();
  vi.mocked(peekInboxList).mockClear();
  root = createRoot(container);
  function Harness() {
    useInboxActivity([], cwd, []);
    return null;
  }
  await act(async () => root.render(createElement(Harness)));
  const hookKeys = vi.mocked(listInboxItems).mock.calls.map(keyOf);
  expect(hookKeys.length).toBeGreaterThan(0);

  // Hidden space 902 is part of every key, and all of them are identical.
  expect(viewKeys.every((key) => key.endsWith(":902"))).toBe(true);
  expect(new Set([...viewKeys, ...hookKeys]).size).toBe(1);
});

it("refetches with the new space ids when the hidden spaces change", async () => {
  await mount();
  expect(issueCalls().at(-1)?.[1]).toMatchObject({ spaceIds: [] });
  const before = issueCalls().length;
  await act(async () => saveHiddenClickUpSpaceIds(["901"]));
  expect(issueCalls().length).toBeGreaterThan(before);
  expect(issueCalls().at(-1)?.[1]).toMatchObject({ spaceIds: ["902"] });
  expect(container.textContent).toContain("Logo");
  expect(container.textContent).not.toContain("Fix export");
});

it("shows the filter badge and a space checklist for the ClickUp source", async () => {
  saveHiddenClickUpSpaceIds(["902"]);
  await mount();
  const filters = container.querySelector<HTMLButtonElement>(
    'button[aria-label^="Filter"]',
  );
  expect(filters).not.toBeNull();
  await act(async () => filters!.click());
  const menu = document.body.querySelector('[role="menu"]')!;
  expect(menu.textContent).toContain("Assigned to me (newest 100)");
  expect(menu.textContent).toContain("Spaces");
  const items = [...menu.querySelectorAll('[role="menuitemcheckbox"]')];
  const design = items.find((entry) => entry.textContent === "Design")!;
  const engineering = items.find(
    (entry) => entry.textContent === "Engineering",
  )!;
  expect(design.getAttribute("aria-checked")).toBe("false");
  expect(engineering.getAttribute("aria-checked")).toBe("true");
  expect(menu.textContent).toContain("Clear filters");
  await act(async () => (design as HTMLButtonElement).click());
  expect(loadHiddenClickUpSpaceIds()).toEqual([]);
});
