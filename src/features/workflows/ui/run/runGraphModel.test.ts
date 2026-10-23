import { expect, it } from "vitest";
import type { NodeRun, Workflow, WorkflowNode, WorkflowRun } from "../../model/workflow";
import { NODE_H, NODE_W } from "../board/boardModel";
import { MANUAL, REPORT, run, workflow } from "../workflowUiFixtures";
import {
  defaultRunView,
  groupViews,
  nodeRunsByKey,
  runGraph,
  type RunNodeStatus,
} from "./runGraphModel";

const session = (id: string, extra: Partial<WorkflowNode> = {}): WorkflowNode =>
  ({
    id,
    type: "session",
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

const stateOf = (nodes: ReturnType<typeof runGraph>["nodes"], id: string) =>
  nodes.find((n) => n.id === id)?.data.run;
const edgeOf = (edges: ReturnType<typeof runGraph>["edges"], from: string, to: string, handle?: string) =>
  edges.find((e) => e.source === from && e.target === to && (e.sourceHandle ?? undefined) === handle);

/** trigger -> dev -> report, no board positions. */
const linear: Workflow = workflow({
  nodes: [MANUAL, session("dev"), REPORT],
  edges: [
    { from: "trigger", to: "dev" },
    { from: "dev", to: "report" },
  ],
});

/** trigger -> each (dev -> review inside) -> report. */
function eachWorkflow(): Workflow {
  return workflow({
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
}

function eachRun(
  statuses: RunNodeStatus[] | number,
  extra: Partial<WorkflowRun> = {},
): WorkflowRun {
  const list = typeof statuses === "number" ? Array<RunNodeStatus>(statuses).fill("pending") : statuses;
  return run({
    definition: eachWorkflow(),
    status: "running",
    items: { each: list.map((_, i) => ({ title: `Ticket ${i + 1}` })) },
    nodes: [
      nr("trigger"),
      nr("each", { status: "running" }),
      ...list.flatMap((status, i) => [
        nr("dev", { branch: `each#${i}`, status }),
        nr("review", { branch: `each#${i}`, status }),
      ]),
      nr("report", { status: "pending" }),
    ],
    ...extra,
  });
}

const children = (nodes: ReturnType<typeof runGraph>["nodes"]) => nodes.filter((n) => n.parentId);

it("linear run maps statuses and labels", () => {
  const r = run({
    definition: linear,
    nodes: [
      nr("trigger"),
      nr("dev", { startedAt: 1000, endedAt: 13000 }),
      nr("report", { status: "pending" }),
    ],
  });
  const { nodes, edges } = runGraph(r, "lanes", groupViews(r));
  expect(stateOf(nodes, "trigger")?.status).toBe("done");
  expect(stateOf(nodes, "dev")?.status).toBe("done");
  expect(stateOf(nodes, "dev")?.label).toMatch(/^Done/);
  expect(stateOf(nodes, "dev")?.label).toContain("12s");
  expect(stateOf(nodes, "dev")?.nodeRunKey).toBe("dev|");
  expect(stateOf(nodes, "report")?.status).toBe("pending");
  expect(edgeOf(edges, "trigger", "dev")?.data?.run).toBe("taken");
  expect(edgeOf(edges, "dev", "report")?.data?.run).toBe("idle");
});

it("if marks only the taken side", () => {
  const r = run({
    definition: workflow({
      nodes: [
        MANUAL,
        { id: "check", type: "if", condition: "{{trigger.x}}" },
        session("yes"),
        session("no"),
      ],
      edges: [
        { from: "trigger", to: "check" },
        { from: "check", to: "yes", handle: "true" },
        { from: "check", to: "no", handle: "false" },
      ],
    }),
    nodes: [
      nr("trigger"),
      nr("check", { decision: { value: "true" } }),
      nr("yes", { status: "pending" }),
      nr("no", { status: "skipped" }),
    ],
  });
  const { nodes, edges } = runGraph(r, "lanes", groupViews(r));
  expect(edgeOf(edges, "check", "yes", "true")?.data?.run).toBe("taken");
  expect(edgeOf(edges, "check", "no", "false")?.data?.run).toBe("idle");
  expect(stateOf(nodes, "no")?.status).toBe("skipped");
  expect(stateOf(nodes, "check")?.label).toBe("Done · true");
});

it("running node and active edge", () => {
  const r = run({
    definition: linear,
    status: "running",
    nodes: [
      nr("trigger"),
      nr("dev", { status: "running", startedAt: Date.now() - 180_000 }),
      nr("report", { status: "pending" }),
    ],
  });
  const { nodes, edges } = runGraph(r, "lanes", groupViews(r));
  expect(stateOf(nodes, "dev")?.status).toBe("running");
  expect(stateOf(nodes, "dev")?.label).toBe("Running 3m");
  expect(edgeOf(edges, "trigger", "dev")?.data?.run).toBe("active");
});

it("asking a loop question shows waiting", () => {
  const r = run({
    definition: linear,
    status: "running",
    nodes: [
      nr("trigger"),
      nr("dev", { status: "running", startedAt: 1, endedAt: 2 }),
      nr("report", { status: "pending" }),
    ],
  });
  const { nodes } = runGraph(r, "lanes", groupViews(r));
  expect(stateOf(nodes, "dev")?.status).toBe("waiting");
});

it("waiting for approval and a failure show their own state", () => {
  const r = run({
    definition: linear,
    status: "running",
    nodes: [
      nr("trigger"),
      nr("dev", { status: "failed", error: "Exit code 2" }),
      nr("report", { status: "waitingApproval" }),
    ],
  });
  const { nodes } = runGraph(r, "lanes", groupViews(r));
  expect(stateOf(nodes, "dev")).toMatchObject({ status: "failed", label: "Failed: Exit code 2" });
  expect(stateOf(nodes, "report")?.status).toBe("waiting");
});

it("lanes view fans a For each into one lane per item", () => {
  const r = eachRun(["done", "running", "failed", "pending"]);
  const { nodes, edges } = runGraph(r, "lanes", groupViews(r));
  const kids = children(nodes);
  expect(kids).toHaveLength(8);
  expect(new Set(kids.map((n) => n.id)).size).toBe(8);
  expect(stateOf(nodes, "dev@each#0")?.status).toBe("done");
  expect(stateOf(nodes, "review@each#1")?.status).toBe("running");
  expect(stateOf(nodes, "dev@each#2")?.status).toBe("failed");
  expect(stateOf(nodes, "dev@each#3")?.status).toBe("pending");
  expect(stateOf(nodes, "dev@each#2")?.nodeRunKey).toBe("dev|each#2");
  expect(kids.every((n) => n.parentId === "each")).toBe(true);
  // One dev -> review edge per lane, plus the two top-level ones.
  expect(edges.filter((e) => e.source.startsWith("dev@"))).toHaveLength(4);
  const group = nodes.find((n) => n.id === "each");
  const lone = runGraph(eachRun(1), "lanes", groupViews(eachRun(1))).nodes.find((n) => n.id === "each");
  expect((group?.width ?? 0) > (lone?.width ?? 0)).toBe(true);
  // Lanes sit side by side without overlapping.
  const xs = [0, 1, 2, 3].map((i) => nodes.find((n) => n.id === `dev@each#${i}`)?.position.x ?? 0);
  for (let i = 1; i < xs.length; i++) expect(xs[i] - xs[i - 1]).toBeGreaterThanOrEqual(NODE_W);
});

it("single view shows only the selected item", () => {
  const r = eachRun(["done", "running", "failed", "pending"]);
  const groups = groupViews(r, { each: { item: 2 } });
  expect(groups[0].item).toBe(2);
  const { nodes } = runGraph(r, "single", groups);
  const kids = children(nodes);
  expect(kids.map((n) => n.id).sort()).toEqual(["dev@each#2", "review@each#2"]);
});

it("picks the first running item by default", () => {
  const r = eachRun(["done", "running", "failed", "pending"]);
  expect(groupViews(r)[0].item).toBe(1);
  expect(groupViews(eachRun(["done", "done"]))[0].item).toBe(0);
});

it("rounds of a For each inside a loop", () => {
  const r = eachRun(["running", "pending"], {
    items: {
      each: [{ title: "New A" }, { title: "New B" }],
      "each@r1": [{ title: "Old A" }],
    },
  });
  r.nodes.find((n) => n.nodeId === "each")!.round = 2;
  r.nodes.push(
    nr("dev", { branch: "each#0@r1", status: "done" }),
    nr("review", { branch: "each#0@r1", status: "failed", error: "nope" }),
  );
  const [group] = groupViews(r);
  expect(group.rounds).toEqual([1, 2]);
  expect(group.round).toBe(2);
  expect(group.items.map((i) => i.title)).toEqual(["New A", "New B"]);

  const [old] = groupViews(r, { each: { round: 1 } });
  expect(old.round).toBe(1);
  expect(old.items).toEqual([{ index: 0, title: "Old A", status: "failed" }]);
  const { nodes } = runGraph(r, "lanes", [old]);
  expect(children(nodes).map((n) => n.id).sort()).toEqual(["dev@each#0@r1", "review@each#0@r1"]);
  expect(stateOf(nodes, "review@each#0@r1")?.nodeRunKey).toBe("review|each#0@r1");
});

it("titles an item by name, else by place", () => {
  const r = eachRun(3, { items: { each: [{ name: " Billing " }, "plain", { title: "" }] } });
  expect(groupViews(r)[0].items.map((i) => i.title)).toEqual(["Billing", "#2", "#3"]);
});

it("summarises the items of a group", () => {
  const r = eachRun(["done", "done", "running", "failed"]);
  expect(groupViews(r)[0].summary).toBe("2 done · 1 running · 1 failed");
});

it("defaults to single view for many items", () => {
  expect(defaultRunView(eachRun(30))).toBe("single");
  expect(defaultRunView(eachRun(3))).toBe("lanes");
  expect(defaultRunView(run({ definition: linear }))).toBe("lanes");
  const wide = eachRun(2);
  wide.definition.nodes.push(session("extra", { parent: "each" }), session("more", { parent: "each" }));
  expect(defaultRunView(wide)).toBe("single");
});

it("empty and failed For each have no lanes", () => {
  const empty = eachRun(0);
  empty.nodes.find((n) => n.nodeId === "each")!.status = "done";
  const [group] = groupViews(empty);
  expect(group.summary).toBe("No items");
  expect(group.items).toEqual([]);
  expect(children(runGraph(empty, "lanes", [group]).nodes)).toHaveLength(0);

  const failed = run({
    definition: eachWorkflow(),
    items: {},
    nodes: [nr("trigger"), nr("each", { status: "failed", error: "Expected a list for {{trigger.tickets}}" })],
  });
  const [bad] = groupViews(failed);
  expect(bad.summary).toBe("Expected a list for {{trigger.tickets}}");
  expect(bad.rounds).toEqual([]);
  expect(children(runGraph(failed, "single", [bad]).nodes)).toHaveLength(0);
});

it("places nodes without positions like the board", () => {
  for (const view of ["lanes", "single"] as const) {
    const r = eachRun(["done", "running", "pending"]);
    const { nodes } = runGraph(r, view, groupViews(r));
    const top = nodes.filter((n) => !n.parentId);
    expect(top).toHaveLength(3);
    for (const n of top) {
      expect(Number.isFinite(n.position.x) && Number.isFinite(n.position.y)).toBe(true);
    }
    const box = (n: (typeof top)[number]) => ({
      x: n.position.x,
      y: n.position.y,
      w: n.width ?? NODE_W,
      h: n.height ?? NODE_H,
    });
    for (let i = 0; i < top.length; i++) {
      for (let j = i + 1; j < top.length; j++) {
        const a = box(top[i]);
        const b = box(top[j]);
        const apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
        expect(apart).toBe(true);
      }
    }
  }
});

it("keeps a widened group clear of a sibling beside it", () => {
  const w = eachWorkflow();
  w.nodes = w.nodes.map((n) =>
    n.id === "each" ? { ...n, ui: { x: 0, y: 100 } } : n.id === "report" ? { ...n, ui: { x: 300, y: 100 } } : n,
  );
  const r = eachRun(["done", "done", "done"], { definition: w });
  const { nodes } = runGraph(r, "lanes", groupViews(r));
  const group = nodes.find((n) => n.id === "each");
  const report = nodes.find((n) => n.id === "report");
  expect((report?.position.x ?? 0) >= (group?.position.x ?? 0) + (group?.width ?? 0)).toBe(true);
});

it("renders a run saved before history existed", () => {
  // No history, no ui positions, and no items entry for the unexpanded group.
  const r = run({
    definition: eachWorkflow(),
    items: {},
    nodes: [nr("trigger"), nr("each", { status: "pending" }), nr("report", { status: "pending" })],
  });
  for (const view of ["lanes", "single"] as const) {
    expect(() => runGraph(r, view, groupViews(r))).not.toThrow();
  }
  expect(groupViews(r)[0].rounds).toEqual([]);
  expect(nodeRunsByKey(r).get("each|")?.status).toBe("pending");
});

it("copes with a run that has fewer node runs than the definition", () => {
  const r = run({ definition: linear, nodes: [] });
  const { nodes } = runGraph(r, "lanes", groupViews(r));
  expect(stateOf(nodes, "dev")).toMatchObject({ status: "pending", round: 1 });
});

it("shows the round on a node that looped and a loop edge as taken", () => {
  const w = workflow({
    nodes: [MANUAL, session("dev"), session("review")],
    edges: [
      { from: "trigger", to: "dev" },
      { from: "dev", to: "review" },
      { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
    ],
  });
  const r = run({
    definition: w,
    status: "running",
    nodes: [
      nr("trigger"),
      nr("dev", { status: "running", round: 2, startedAt: Date.now() }),
      nr("review", {
        status: "pending",
        round: 2,
        history: [{ round: 1, status: "done", decision: { value: "loop", reason: "needs tests" } }],
      }),
    ],
  });
  const { nodes, edges } = runGraph(r, "lanes", groupViews(r));
  expect(stateOf(nodes, "dev")?.round).toBe(2);
  expect(stateOf(nodes, "dev")?.label).toContain("round 2");
  const loop = edges.find((e) => e.data?.loop);
  expect(loop?.data?.run).toBe("taken");
  expect(nodes.find((n) => n.id === "review")?.data.loopOut).toBe(true);
  expect(nodes.find((n) => n.id === "dev")?.data.loopIn).toBe(true);
});
