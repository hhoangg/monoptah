// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { habitScheduleLabel } from "../../monos/model/monoHabits";
import type { Workflow, WorkflowRun } from "../model/workflow";
import { setWorkflowRunning } from "../model/workflowStore";
import {
  WorkflowsList,
  useWorkflows,
  workflowActions,
  type WorkflowActions,
} from "./MonoWorkflows";
import { SCHEDULE, run, scheduled, workflow } from "./workflowUiFixtures";

/** The Mono's folder, in memory: workflows.json and the runs files. */
const files = new Map<string, string>();
let runs: WorkflowRun[] = [];
let loadCount = 0;

vi.mock("../../monos/model/monoFiles", async (original) => ({
  ...(await original<object>()),
  readAgentFile: async (_monoId: string, path: string) => {
    if (path === "workflows.json") loadCount++;
    if (path.startsWith("workflow-runs/"))
      return { text: JSON.stringify(runs), hash: "r" };
    return { text: files.get(path) ?? null, hash: String(files.get(path)) };
  },
  writeAgentFile: async (_monoId: string, path: string, text: string) => {
    files.set(path, text);
    window.dispatchEvent(new CustomEvent("monocode:mono-files-changed"));
    return "h";
  },
}));

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  files.clear();
  runs = [];
  loadCount = 0;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const save = (list: Workflow[]) =>
  files.set("workflows.json", JSON.stringify(list));
const saved = (): Workflow[] => JSON.parse(files.get("workflows.json") ?? "[]");

/** The tab as the overview mounts it: the Mono's workflows, loaded, in the list. */
function Panel({ actions }: { actions: WorkflowActions }) {
  const workflows = useWorkflows("mono-1", "idle");
  return createElement(WorkflowsList, {
    monoId: "mono-1",
    workflows,
    actions,
    onOpen: () => {},
    empty: createElement(
      "p",
      null,
      "Ask Broski to chain sessions into a workflow, or create one with +.",
    ),
  });
}

async function renderPanel(
  actions: WorkflowActions = workflowActions("mono-1"),
) {
  await act(async () =>
    root.render(createElement(Panel, { actions })),
  );
}

const rows = () => [...container.querySelectorAll("li[data-workflow]")];
const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;

it("tab lists workflows with schedule label", async () => {
  save([
    scheduled({ id: "wf-schedule", name: "Morning checks" }),
    workflow({ id: "wf-manual", name: "Release train" }),
  ]);
  await renderPanel();
  expect(rows().map((row) => row.getAttribute("data-workflow"))).toEqual([
    "wf-schedule",
    "wf-manual",
  ]);
  expect(rows()[0].textContent).toContain("Morning checks");
  expect(rows()[0].textContent).toContain(habitScheduleLabel(SCHEDULE));
  expect(rows()[1].textContent).toContain("Manual");
});

it("says how to fill the tab when there are no workflows", async () => {
  save([]);
  await renderPanel();
  expect(container.textContent).toContain(
    "Ask Broski to chain sessions into a workflow, or create one with +.",
  );
});

it("paused row is dimmed and says Paused", async () => {
  save([scheduled({ enabled: false })]);
  await renderPanel();
  const row = rows()[0];
  expect(row.textContent).toContain(
    `Paused · ${habitScheduleLabel(SCHEDULE)}`,
  );
  expect(row.querySelector(".text-content\\/45")?.textContent).toBe(
    "Nightly checks",
  );
  expect(button("Resume")).not.toBeNull();
});

it("a running row counts its branches and offers Stop", async () => {
  save([workflow()]);
  runs = [
    run({
      id: "run-live",
      status: "running",
      startedAt: Date.now() - 12 * 60_000,
      nodes: [
        { nodeId: "a", branch: "each#0", status: "running", round: 1 },
        { nodeId: "a", branch: "each#1", status: "running", round: 1 },
        { nodeId: "b", branch: "each#2", status: "waitingApproval", round: 1 },
        { nodeId: "c", branch: "each#3", status: "done", round: 1 },
      ],
    }),
  ];
  act(() => setWorkflowRunning("wf-aaaaaaaa", "run-live"));
  await renderPanel();
  expect(rows()[0].textContent).toContain("Running · 3 branches · 12m");
  expect(button("Stop")).not.toBeNull();
  act(() => setWorkflowRunning("wf-aaaaaaaa", null));
});

it("Run now asks for a run", async () => {
  save([workflow()]);
  await renderPanel();
  await act(async () => button("Run now").click());
  expect(saved()[0].runRequested).toBe(true);
});

it("Run now refuses a workflow with issues and says why", async () => {
  save([workflow({ nodes: [], edges: [] })]);
  await renderPanel();
  await act(async () => button("Run now").click());
  expect(saved()[0].runRequested).toBeUndefined();
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toMatch(
    /Can't run/,
  );
});

it("Resume schedules the next run, and refuses when the workflow has issues", async () => {
  save([scheduled({ enabled: false, nextRunAt: undefined })]);
  await renderPanel();
  await act(async () => button("Resume").click());
  expect(saved()[0].enabled).toBe(true);
  expect(saved()[0].nextRunAt).toBeGreaterThan(Date.now());

  save([workflow({ enabled: false, nodes: [], edges: [] })]);
  await act(async () => window.dispatchEvent(new CustomEvent("focus")));
  await act(async () => button("Resume").click());
  expect(saved()[0].enabled).toBe(false);
  expect(container.querySelector("[data-workflow-notice]")).not.toBeNull();
});

it("Remove takes the workflow out of the file", async () => {
  save([workflow()]);
  await renderPanel();
  await act(async () => button("Remove").click());
  expect(saved()).toEqual([]);
});

it("refreshes when the Mono changes the workflows file", async () => {
  save([workflow({ name: "First" })]);
  await renderPanel();
  expect(rows()).toHaveLength(1);
  const before = loadCount;
  save([workflow({ name: "First" }), workflow({ id: "wf-bbbbbbbb", name: "Second" })]);
  await act(async () => {
    window.dispatchEvent(new CustomEvent("monocode:mono-files-changed"));
  });
  expect(loadCount).toBeGreaterThan(before);
  expect(rows().map((row) => row.textContent)).toEqual([
    expect.stringContaining("First"),
    expect.stringContaining("Second"),
  ]);
});
