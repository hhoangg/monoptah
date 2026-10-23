// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { NodeRun, Workflow, WorkflowNode, WorkflowRun } from "../../model/workflow";
import { WorkflowRunPage } from "../WorkflowRunPage";
import { MANUAL, REPORT, run, workflow } from "../workflowUiFixtures";

let stored: WorkflowRun[] = [];
const stopWorkflowRun = vi.hoisted(() => vi.fn(async () => {}));

vi.mock("../../../../app/hooks/useMonoWorkflows", () => ({ resumeWorkflowRun: vi.fn(async () => {}) }));
vi.mock("../../model/workflowStore", async (original) => ({
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

const scrollIntoView = vi.fn();
let root: Root;
let container: HTMLElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", FakeObserver);
  vi.stubGlobal("DOMMatrixReadOnly", FakeMatrix);
  Element.prototype.scrollIntoView = scrollIntoView;
  scrollIntoView.mockClear();
  localStorage.clear();
  stored = [];
  container = document.createElement("div");
  container.style.width = "420px";
  container.style.height = "800px";
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const session = (id: string, extra: Partial<WorkflowNode> = {}): WorkflowNode =>
  ({
    id,
    type: "session",
    title: id === "dev" ? "Dev" : "Review",
    project: "{{inputs.project}}",
    prompt: "Do it",
    checkout: "root",
    output: {},
    ...extra,
  }) as WorkflowNode;

const nr = (nodeId: string, extra: Partial<NodeRun> = {}): NodeRun => ({
  nodeId,
  branch: "",
  status: "done",
  round: 1,
  ...extra,
});

const linear: Workflow = workflow({
  nodes: [MANUAL, session("dev"), REPORT],
  edges: [
    { from: "trigger", to: "dev" },
    { from: "dev", to: "report" },
  ],
});

/** trigger -> each (dev -> review inside) -> report. */
const eachDefinition: Workflow = workflow({
  nodes: [
    MANUAL,
    { id: "each", type: "forEach", in: "{{trigger.tickets}}", title: "Tickets" },
    session("dev", { parent: "each" }),
    session("review", { parent: "each" }),
    REPORT,
  ],
  edges: [
    { from: "trigger", to: "each" },
    { from: "dev", to: "review" },
    { from: "each", to: "report" },
  ],
});

function eachRun(count: number, extra: Partial<WorkflowRun> = {}): WorkflowRun {
  return run({
    definition: eachDefinition,
    status: "running",
    items: { each: Array.from({ length: count }, (_, i) => ({ title: `Ticket ${i + 1}` })) },
    nodes: [
      nr("trigger"),
      nr("each", { status: "running" }),
      ...Array.from({ length: count }, (_, i) => [
        nr("dev", { branch: `each#${i}`, status: i === 0 ? "running" : "pending" }),
        nr("review", { branch: `each#${i}`, status: "pending" }),
      ]).flat(),
      nr("report", { status: "pending" }),
    ],
    ...extra,
  });
}

async function renderPage(current: Workflow | undefined, onOpenSession = vi.fn()) {
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

const cards = () => [...container.querySelectorAll<HTMLElement>("[data-board-node]")];
const laneCards = () => cards().filter((c) => c.getAttribute("data-board-node")?.includes("@"));
const viewSwitch = () =>
  container.querySelector<HTMLButtonElement>(
    'button[aria-label="Show one item at a time"], button[aria-label="Show items side by side"]',
  );

it("run page draws one card per run node", async () => {
  stored = [
    run({
      definition: linear,
      status: "running",
      nodes: [nr("trigger"), nr("dev"), nr("report", { status: "pending" })],
    }),
  ];
  await renderPage(linear);
  expect(cards()).toHaveLength(3);
  expect(cards().map((c) => c.getAttribute("data-run-status"))).toEqual(["done", "done", "pending"]);
});

it("switch toggles lanes and single and is remembered", async () => {
  stored = [eachRun(4)];
  await renderPage(eachDefinition);
  expect(laneCards()).toHaveLength(8);
  await act(async () => viewSwitch()!.click());
  expect(laneCards()).toHaveLength(2);
  expect(localStorage.getItem("monocode.workflowRunView.wf-aaaaaaaa")).toBe("single");
  act(() => root.unmount());
  root = createRoot(container);
  await renderPage(eachDefinition);
  expect(laneCards()).toHaveLength(2);
  await act(async () => viewSwitch()!.click());
  expect(laneCards()).toHaveLength(8);
});

it("no switch without a For each", async () => {
  stored = [run({ definition: linear, nodes: [nr("trigger"), nr("dev"), nr("report")] })];
  await renderPage(linear);
  expect(viewSwitch()).toBeNull();
});

it("tabs scroll horizontally", async () => {
  stored = [eachRun(30)];
  await renderPage(eachDefinition);
  // Many items default to the single lane view.
  expect(laneCards()).toHaveLength(2);
  const tabs = container.querySelector<HTMLElement>("[data-run-tabs]")!;
  expect(tabs).not.toBeNull();
  expect(getComputedStyle(tabs).overflowX).toBe("auto");
  expect(tabs.querySelectorAll("[data-run-tab]")).toHaveLength(30);
  expect(scrollIntoView).toHaveBeenCalled();
  expect(tabs.querySelector("[data-run-tab][aria-selected='true']")?.textContent).toContain("Ticket 1");
  // Picking another item moves the selected tab and shows its lane.
  scrollIntoView.mockClear();
  await act(async () => tabs.querySelectorAll<HTMLElement>("[data-run-tab]")[7].click());
  expect(tabs.querySelector("[data-run-tab][aria-selected='true']")?.textContent).toContain("Ticket 8");
  expect(laneCards().map((c) => c.getAttribute("data-board-node"))).toEqual([
    "dev@each#7",
    "review@each#7",
  ]);
  expect(scrollIntoView).toHaveBeenCalled();
});

it("clicking a node shows its details and round history", async () => {
  const withHistory = run({
    definition: linear,
    status: "running",
    nodes: [
      nr("trigger"),
      nr("dev", {
        status: "running",
        round: 2,
        startedAt: Date.now() - 5000,
        history: [
          {
            round: 1,
            status: "done",
            sessionId: "s-dev-1",
            decision: { value: "loop", reason: "needs tests" },
            startedAt: 1000,
            endedAt: 13000,
          },
        ],
      }),
      nr("report", { status: "pending" }),
    ],
  });
  stored = [withHistory];
  const onOpenSession = await renderPage(linear);
  await act(async () => cards().find((c) => c.getAttribute("data-board-node") === "dev")!.click());
  const details = container.querySelector("[data-run-details]")!;
  expect(details.textContent).toContain("Dev");
  expect(details.textContent).toContain("Session");
  const record = details.querySelector("[data-round-record='1']")!;
  expect(record.textContent).toContain("Round 1");
  expect(record.textContent).toContain("needs tests");
  const open = [...record.querySelectorAll("button")].find((b) => b.textContent === "Open session")!;
  await act(async () => open.click());
  expect(onOpenSession).toHaveBeenCalledWith("s-dev-1");
});

it("details start on the running node and truncate long output with Show more", async () => {
  const long = "x".repeat(500);
  stored = [
    run({
      definition: linear,
      status: "running",
      nodes: [
        nr("trigger"),
        nr("dev", { status: "running", output: { summary: long, pr: "u1" } }),
        nr("report", { status: "pending" }),
      ],
    }),
  ];
  await renderPage(linear);
  const details = container.querySelector("[data-run-details]")!;
  expect(details.textContent).toContain("Dev");
  expect(details.textContent).toContain("u1");
  expect(details.textContent).not.toContain(long);
  const more = [...details.querySelectorAll("button")].find((b) => b.textContent === "Show more")!;
  await act(async () => more.click());
  expect(details.textContent).toContain(long);
});

it("shows a command's exit code and output tails", async () => {
  const command: Workflow = workflow({
    nodes: [MANUAL, { id: "lint", type: "command", command: "npm run lint", project: "{{inputs.project}}" } as WorkflowNode],
    edges: [{ from: "trigger", to: "lint" }],
  });
  stored = [
    run({
      definition: command,
      status: "failed",
      nodes: [
        nr("trigger"),
        nr("lint", { status: "failed", error: "exit 2", output: { exitCode: 2, stdout: "ok so far", stderr: "boom" } }),
      ],
    }),
  ];
  await renderPage(command);
  const details = container.querySelector("[data-run-details]")!;
  expect(details.textContent).toContain("Exit code");
  expect(details.textContent).toContain("2");
  expect(details.textContent).toContain("ok so far");
  expect(details.textContent).toContain("boom");
});

it("keeps stop and the changed note", async () => {
  stored = [eachRun(2)];
  await renderPage(eachDefinition);
  const stop = container.querySelector<HTMLButtonElement>('button[aria-label="Stop run"]')!;
  expect(stop).not.toBeNull();
  await act(async () => stop.click());
  expect(stopWorkflowRun).toHaveBeenCalledWith("mono-1", "wf-aaaaaaaa");
  expect(container.textContent).not.toContain("changed since this run started");
  await renderPage({ ...eachDefinition, edges: [] });
  expect(container.textContent).toContain("The workflow changed since this run started.");
});

it("renders a run with no nodes recorded yet", async () => {
  stored = [run({ definition: linear, status: "running", nodes: [] })];
  await renderPage(linear);
  expect(cards()).toHaveLength(3);
  expect(cards().map((c) => c.getAttribute("data-run-status"))).toEqual(["pending", "pending", "pending"]);
});
