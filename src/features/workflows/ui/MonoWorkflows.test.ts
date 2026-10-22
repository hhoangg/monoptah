// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { habitScheduleLabel } from "../../monos/model/monoHabits";
import { definitionFingerprint, type Workflow, type WorkflowRun } from "../model/workflow";
import { setWorkflowRunning } from "../model/workflowStore";
import {
  WorkflowsList,
  useWorkflows,
  type ReviewIntent,
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
  onReview.mockReset();
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
function Panel({
  actions,
  onReview,
}: {
  actions: WorkflowActions;
  onReview: (id: string, intent: ReviewIntent) => void;
}) {
  const workflows = useWorkflows("mono-1", "idle");
  return createElement(WorkflowsList, {
    monoId: "mono-1",
    workflows,
    actions,
    onOpen: () => {},
    onReview,
    empty: createElement(
      "p",
      null,
      "Ask Broski to chain sessions into a workflow, or create one with +.",
    ),
  });
}

const onReview = vi.fn<(id: string, intent: ReviewIntent) => void>();

async function renderPanel(
  actions: WorkflowActions = workflowActions("mono-1"),
) {
  await act(async () =>
    root.render(createElement(Panel, { actions, onReview })),
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

it("Remove also empties the workflow's run history", async () => {
  save([workflow()]);
  files.set("workflow-runs/wf-aaaaaaaa.json", '[{"id":"run-1"}]');
  await renderPanel();
  await act(async () => button("Remove").click());
  expect(saved()).toEqual([]);
  expect(files.get("workflow-runs/wf-aaaaaaaa.json")).toBe("[]\n");
});

it("Remove is refused while a run is going", async () => {
  save([workflow()]);
  files.set("workflow-runs/wf-aaaaaaaa.json", '[{"id":"run-1"}]');
  act(() => setWorkflowRunning("wf-aaaaaaaa", "run-live"));
  await renderPanel();
  await act(async () => button("Remove").click());
  act(() => setWorkflowRunning("wf-aaaaaaaa", null));
  expect(saved()).toHaveLength(1);
  expect(files.get("workflow-runs/wf-aaaaaaaa.json")).toBe('[{"id":"run-1"}]');
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toMatch(
    /Stop the workflow before removing it/,
  );
});

it("a row that needs review says so, and others do not", async () => {
  save([
    workflow({ id: "wf-flagged1", name: "Imported", needsReview: true }),
    workflow({ id: "wf-plain001", name: "Mine" }),
  ]);
  await renderPanel();
  const [flagged, plain] = rows();
  expect(flagged.querySelector("[data-needs-review]")?.textContent).toBe("Needs review");
  expect(plain.querySelector("[data-needs-review]")).toBeNull();
});

it("Resume on a workflow that needs review opens the review instead of enabling it", async () => {
  save([scheduled({ enabled: false, nextRunAt: undefined, needsReview: true })]);
  await renderPanel();
  await act(async () => button("Resume").click());
  expect(onReview).toHaveBeenCalledWith("wf-aaaaaaaa", "enable");
  expect(saved()[0]).toMatchObject({ enabled: false, needsReview: true });
  expect(saved()[0].nextRunAt).toBeUndefined();
});

it("Run now on a workflow that needs review opens the review instead of running it", async () => {
  save([workflow({ enabled: false, needsReview: true })]);
  await renderPanel();
  await act(async () => button("Run now").click());
  expect(onReview).toHaveBeenCalledWith("wf-aaaaaaaa", "run");
  expect(saved()[0].runRequested).toBeUndefined();
  expect(saved()[0].needsReview).toBe(true);
});

it("the actions themselves refuse a flagged workflow, whoever calls them", async () => {
  save([workflow({ enabled: false, needsReview: true })]);
  const actions = workflowActions("mono-1");
  expect(await actions.runNow("wf-aaaaaaaa")).toMatch(/Review this workflow/);
  expect(await actions.toggle("wf-aaaaaaaa")).toMatch(/Review this workflow/);
  expect(saved()[0]).toMatchObject({ enabled: false, needsReview: true });
  expect(saved()[0].runRequested).toBeUndefined();
});

describe("approve", () => {
  const fingerprint = (w: Workflow) => definitionFingerprint(w);

  it("clears the mark and enables the workflow, counting the next run from now", async () => {
    const w = scheduled({ enabled: false, nextRunAt: undefined, needsReview: true });
    save([w]);
    expect(await workflowActions("mono-1").approve(w.id, "enable", fingerprint(w))).toEqual({
      kind: "done",
    });
    expect(saved()[0].enabled).toBe(true);
    expect(saved()[0].needsReview).toBeUndefined();
    expect(saved()[0].nextRunAt).toBeGreaterThan(Date.now());
  });

  it("clears the mark and asks for a run", async () => {
    const w = workflow({ enabled: false, needsReview: true });
    save([w]);
    expect(await workflowActions("mono-1").approve(w.id, "run", fingerprint(w))).toEqual({
      kind: "done",
    });
    expect(saved()[0]).toMatchObject({ runRequested: true, enabled: false });
    expect(saved()[0].needsReview).toBeUndefined();
  });

  it("changes nothing when the stored definition is not the one that was reviewed", async () => {
    const reviewed = workflow({
      enabled: false,
      needsReview: true,
      nodes: [
        { id: "trigger", type: "manual" },
        { id: "build", type: "command", project: "{{inputs.project}}", command: "npm test" },
        { id: "report", type: "report" },
      ],
      edges: [
        { from: "trigger", to: "build" },
        { from: "build", to: "report" },
      ],
    });
    const swapped: Workflow = {
      ...reviewed,
      nodes: reviewed.nodes.map((n) =>
        n.type === "command" ? { ...n, command: "rm -rf ~" } : n,
      ),
    };
    save([swapped]);
    const result = await workflowActions("mono-1").approve(
      reviewed.id,
      "enable",
      fingerprint(reviewed),
    );
    expect(result).toMatchObject({ kind: "changed", workflow: { needsReview: true } });
    expect(saved()[0]).toEqual(swapped);
  });

  it("refuses a workflow with problems and keeps the mark", async () => {
    const w = workflow({ enabled: false, nodes: [], edges: [], needsReview: true });
    save([w]);
    const result = await workflowActions("mono-1").approve(w.id, "enable", fingerprint(w));
    expect(result).toMatchObject({ kind: "refused", notice: expect.stringMatching(/Can't resume/) });
    expect(saved()[0]).toMatchObject({ enabled: false, needsReview: true });
  });

  it("does nothing for a workflow that is gone", async () => {
    save([]);
    const result = await workflowActions("mono-1").approve("wf-aaaaaaaa", "run", "x");
    expect(result).toMatchObject({ kind: "refused" });
    expect(saved()).toEqual([]);
  });
});

it("Pause leaves a pending review alone", async () => {
  save([workflow({ needsReview: true })]);
  await renderPanel();
  await act(async () => button("Pause").click());
  expect(saved()[0]).toMatchObject({ enabled: false, needsReview: true });
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
