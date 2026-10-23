// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { NodeRun, Workflow, WorkflowRun } from "../model/workflow";
import { setWorkflowRunning } from "../model/workflowStore";
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

/** React Flow measures with these; happy-dom has neither. */
class FakeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
class FakeMatrix {
  m22 = 1;
  constructor(_transform?: string) {}
}

let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", FakeObserver);
  vi.stubGlobal("DOMMatrixReadOnly", FakeMatrix);
  Element.prototype.scrollIntoView = () => {};
  localStorage.clear();
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

const card = (id: string) =>
  container.querySelector<HTMLElement>(`[data-board-node="${id}"]`);
const laneTitles = () =>
  [...container.querySelectorAll("[data-run-lane-title]")].map((t) => t.textContent);
const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);

it("run page shows one card per item node, titled by item, with round 2", async () => {
  stored = [laneRun()];
  await renderPage();
  expect(
    [...container.querySelectorAll("[data-board-node]")]
      .map((c) => c.getAttribute("data-board-node"))
      .sort(),
  ).toEqual(["dev@each#0", "dev@each#1", "dev@each#2", "each", "report", "trigger"]);
  expect(laneTitles()).toEqual(["Fix login", "Billing", "#3"]);
  // The group box carries the summary, the cards their state.
  expect(card("each")?.textContent).toContain("Tickets");
  expect(card("each")?.textContent).toContain("1 done");
  // The loop round shows only past the first one, with the decision's reason.
  expect(card("dev@each#0")?.textContent).toContain("round 2");
  expect(card("dev@each#0")?.textContent).toContain("tests still failing");
  expect(card("dev@each#1")?.textContent).not.toContain("round");
  expect(card("dev@each#1")?.getAttribute("data-run-status")).toBe("running");
  expect(card("dev@each#2")?.getAttribute("data-run-status")).toBe("waiting");
  expect(card("dev@each#2")?.textContent).toContain("Waiting for approval");
});

it("run page picks between the rounds of a For each a loop ran again", async () => {
  stored = [
    laneRun({
      items: {
        each: [{ title: "Fix login" }, { name: "Billing" }],
        "each@r1": [{ title: "Old login" }, { name: "Old billing" }],
      },
      nodes: [
        node({ nodeId: "trigger" }),
        node({ nodeId: "each", status: "running", round: 2 }),
        node({ branch: "each#0", status: "running", sessionId: "live-0" }),
        node({ branch: "each#1", status: "pending" }),
        node({ branch: "each#0@r1", status: "failed", sessionId: "old-0", error: "boom" }),
        node({ branch: "each#1@r1", sessionId: "old-1" }),
        node({ nodeId: "report", status: "pending" }),
      ],
    }),
  ];
  await renderPage();
  const rounds = () =>
    [...container.querySelectorAll<HTMLElement>("[data-run-rounds] [data-run-tab]")];
  expect(rounds().map((r) => r.textContent)).toEqual(["Round 1", "Round 2"]);
  // The latest round shows by default.
  expect(laneTitles()).toEqual(["Fix login", "Billing"]);
  await act(async () => rounds()[0].click());
  expect(laneTitles()).toEqual(["Old login", "Old billing"]);
  expect(card("dev@each#0@r1")?.getAttribute("data-run-status")).toBe("failed");
});

it("opens a node's session from its details", async () => {
  stored = [laneRun()];
  const onOpenSession = await renderPage();
  await act(async () => card("dev@each#0")!.click());
  const open = [...container.querySelectorAll<HTMLButtonElement>("[data-run-details] button")].find(
    (b) => b.textContent === "Open session",
  )!;
  act(() => open.click());
  expect(onOpenSession).toHaveBeenCalledWith("session-1");
  // A node without a session has nothing to open.
  await act(async () => card("dev@each#1")!.click());
  expect(
    [...container.querySelectorAll("[data-run-details] button")].some(
      (b) => b.textContent === "Open session",
    ),
  ).toBe(false);
});

it("says so when a node's session cannot be opened", async () => {
  stored = [laneRun()];
  const onOpenSession = vi.fn(async () => {
    throw new Error("gone");
  });
  const unhandled = vi.fn();
  process.on("unhandledRejection", unhandled);
  await renderPage(definition, onOpenSession);
  await act(async () => card("dev@each#0")!.click());
  await act(async () =>
    [...container.querySelectorAll<HTMLButtonElement>("[data-run-details] button")]
      .find((b) => b.textContent === "Open session")!
      .click(),
  );
  process.off("unhandledRejection", unhandled);
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toBe(
    "Could not open this session.",
  );
  expect(unhandled).not.toHaveBeenCalled();
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

it("Resume is refused while another run of the workflow is going", async () => {
  stored = [laneRun({ status: "interrupted" })];
  await renderPage();
  act(() => setWorkflowRunning("wf-aaaaaaaa", "run-2"));
  await act(async () => button("Resume")!.click());
  act(() => setWorkflowRunning("wf-aaaaaaaa", null));
  expect(resumeWorkflowRun).not.toHaveBeenCalled();
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toMatch(
    /Another run of this workflow is going/,
  );
});

it("Resume is refused while the workflow still needs its review", async () => {
  stored = [laneRun({ status: "interrupted" })];
  await renderPage({ ...definition, needsReview: true });
  await act(async () => button("Resume")!.click());
  expect(resumeWorkflowRun).not.toHaveBeenCalled();
  expect(container.querySelector("[data-workflow-notice]")?.textContent).toMatch(
    /Review this workflow before resuming its run/,
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
