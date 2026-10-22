// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { NodeRun, Workflow, WorkflowRun } from "../model/workflow";
import { WorkflowRunPage } from "./WorkflowRunPage";
import { MANUAL, REPORT, run, workflow } from "./workflowUiFixtures";

let stored: WorkflowRun[] = [];
const resumeWorkflowRun = vi.hoisted(() => vi.fn(async () => {}));
const stopWorkflowRun = vi.hoisted(() => vi.fn(async () => {}));

vi.mock("../../../app/hooks/useMonoWorkflows", () => ({ resumeWorkflowRun }));
vi.mock("../model/workflowStore", async (original) => ({
  ...(await original<object>()),
  loadRuns: async () => stored,
  stopWorkflowRun,
}));

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  stored = [];
  resumeWorkflowRun.mockClear();
  stopWorkflowRun.mockClear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

/** trigger -> each (over two tickets) -> dev inside it -> report. */
const definition: Workflow = workflow({
  nodes: [
    MANUAL,
    { id: "each", type: "forEach", in: "{{trigger.tickets}}", title: "Tickets" },
    {
      id: "dev",
      type: "session",
      title: "Dev",
      parent: "each",
      project: "{{inputs.project}}",
      prompt: "Fix it",
      checkout: "root",
      output: {},
    },
    REPORT,
  ],
  edges: [{ from: "each", to: "report" }],
});

const node = (extra: Partial<NodeRun>): NodeRun => ({
  nodeId: "dev",
  branch: "",
  status: "done",
  round: 1,
  ...extra,
});

function laneRun(extra: Partial<WorkflowRun> = {}): WorkflowRun {
  return run({
    definition,
    status: "running",
    items: { each: [{ title: "Fix login" }, { name: "Billing" }, {}] },
    nodes: [
      node({ nodeId: "trigger" }),
      node({ nodeId: "each", status: "running" }),
      node({
        branch: "each#0",
        round: 2,
        sessionId: "session-1",
        decision: { value: "loop", reason: "tests still failing" },
      }),
      node({ branch: "each#1", status: "running" }),
      node({ branch: "each#2", status: "waitingApproval" }),
      node({ nodeId: "report", status: "pending" }),
    ],
    ...extra,
  });
}

async function renderPage(
  current: Workflow | undefined = definition,
  onOpenSession = vi.fn(),
) {
  await act(async () =>
    root.render(
      createElement(WorkflowRunPage, {
        monoId: "mono-1",
        workflowId: "wf-aaaaaaaa",
        runId: "run-1",
        current,
        color: "#9c9",
        onBack: () => {},
        onOpenSession,
      }),
    ),
  );
  return onOpenSession;
}

const lanes = () => [...container.querySelectorAll("[data-lane]")];
const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);

it("run page shows one lane per item and round 2", async () => {
  stored = [laneRun()];
  await renderPage();
  expect(lanes().map((lane) => lane.getAttribute("data-lane"))).toEqual([
    "",
    "each#0",
    "each#1",
    "each#2",
  ]);
  expect(
    lanes().map((lane) => lane.querySelector("h4")?.textContent),
  ).toEqual(["Workflow", "Fix login", "Billing", "#3"]);
  // Top level first: the group node, not the trigger.
  expect(lanes()[0].textContent).toContain("Tickets");
  expect(lanes()[0].textContent).not.toContain("trigger");
  // The loop round shows only past the first one, with the decision's reason.
  expect(lanes()[1].textContent).toContain("Dev · round 2");
  expect(lanes()[1].textContent).toContain("tests still failing");
  expect(lanes()[2].textContent).not.toContain("round");
  expect(lanes()[2].textContent).toContain("Running");
  expect(lanes()[3].textContent).toContain("Waiting for approval");
});

it("opens a node's session from its chip", async () => {
  stored = [laneRun()];
  const onOpenSession = await renderPage();
  const chips = container.querySelectorAll<HTMLButtonElement>(
    "[data-lane='each#0'] button[data-node]",
  );
  expect(chips).toHaveLength(1);
  act(() => chips[0].click());
  expect(onOpenSession).toHaveBeenCalledWith("session-1");
  // A node without a session is not a button.
  expect(
    container.querySelectorAll("[data-lane='each#1'] button[data-node]"),
  ).toHaveLength(0);
});

it("running run offers Stop run", async () => {
  stored = [laneRun()];
  await renderPage();
  expect(button("Resume")).toBeNull();
  await act(async () => button("Stop run")!.click());
  expect(stopWorkflowRun).toHaveBeenCalledWith("mono-1", "wf-aaaaaaaa");
});

it("interrupted run shows Resume", async () => {
  stored = [laneRun({ status: "interrupted" })];
  await renderPage();
  expect(button("Stop run")).toBeNull();
  await act(async () => button("Resume")!.click());
  expect(resumeWorkflowRun).toHaveBeenCalledWith(
    "mono-1",
    "wf-aaaaaaaa",
    "run-1",
  );
});

it("notes when the workflow changed since the run started, ignoring board positions", async () => {
  stored = [laneRun({ status: "done" })];
  const moved: Workflow = {
    ...definition,
    nodes: definition.nodes.map((n) => ({ ...n, ui: { x: 5, y: 9 } })),
  };
  await renderPage(moved);
  expect(container.textContent).not.toContain("changed since this run started");
  const edited: Workflow = { ...definition, edges: [] };
  await renderPage(edited);
  expect(container.textContent).toContain(
    "The workflow changed since this run started.",
  );
});
