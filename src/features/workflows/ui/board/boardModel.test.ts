import { describe, expect, it } from "vitest";
import type { Workflow, WorkflowNode } from "../../model/workflow";
import { validateWorkflow } from "../../model/workflowParse";
import { ticketWorkflow, workflow } from "../workflowUiFixtures";
import {
  autoPlace,
  connectionKind,
  definitionKey,
  deletion,
  dropCrossingEdges,
  dropNodes,
  fitGroups,
  fromFlow,
  newNode,
  projectInputFor,
  pushBelow,
  settleDrop,
  toFlow,
  NODE_H,
  NODE_W,
  type FlowNode,
} from "./boardModel";

describe("the fixture", () => {
  it("is a valid workflow", () => {
    expect(validateWorkflow(ticketWorkflow())).toEqual([]);
  });
});

describe("toFlow / fromFlow", () => {
  it("round-trip keeps nodes, edges, parents and positions", () => {
    const w = ticketWorkflow();
    const { nodes, edges } = toFlow(w);
    const back = fromFlow(w, nodes, edges);
    expect(back.nodes).toEqual(w.nodes);
    expect(back.edges).toEqual(w.edges);
    expect(back.nodes.find((n) => n.id === "dev")?.parent).toBe("each");
    expect(back.nodes.find((n) => n.id === "dev")?.ui).toEqual({ x: 16, y: 40 });
  });

  it("makes a For each a group and its children parent-bound, parents first", () => {
    const { nodes } = toFlow(ticketWorkflow());
    const group = nodes.find((n) => n.id === "each");
    const dev = nodes.find((n) => n.id === "dev");
    expect(group?.type).toBe("group");
    expect(dev).toMatchObject({ parentId: "each", extent: "parent", type: "board" });
    expect(nodes.findIndex((n) => n.id === "each")).toBeLessThan(
      nodes.findIndex((n) => n.id === "dev"),
    );
    expect(group?.width).toBeGreaterThan(NODE_W);
  });

  it("draws a loop edge on the side handles with its spec", () => {
    const { edges } = toFlow(ticketWorkflow());
    expect(edges.find((e) => e.type === "loop")).toMatchObject({
      source: "review",
      target: "dev",
      sourceHandle: "loop-out",
      targetHandle: "loop-in",
      data: { loop: { maxRounds: 3, decide: "mono" } },
    });
    const plain = edges.filter((e) => e.type !== "loop");
    expect(plain).toHaveLength(4);
    expect(plain.every((e) => e.sourceHandle === undefined)).toBe(true);
  });

  it("keeps the true and false handles of an If node", () => {
    const w = workflow({
      nodes: [
        { id: "trigger", type: "manual", ui: { x: 0, y: 0 } },
        { id: "check", type: "if", condition: "{{inputs.x}} == 1", ui: { x: 0, y: 80 } },
        { id: "report", type: "report", ui: { x: 0, y: 160 } },
      ],
      edges: [
        { from: "trigger", to: "check" },
        { from: "check", to: "report", handle: "true" },
      ],
    });
    const { nodes, edges } = toFlow(w);
    expect(edges[1]).toMatchObject({ sourceHandle: "true" });
    expect(fromFlow(w, nodes, edges).edges).toEqual(w.edges);
  });

  it("lays out nodes that have no position, top to bottom without overlap", () => {
    const w = ticketWorkflow();
    const bare: Workflow = {
      ...w,
      nodes: w.nodes.map(({ ui: _ui, ...node }) => node as WorkflowNode),
    };
    const { nodes } = toFlow(bare);
    const at = (id: string) => nodes.find((n) => n.id === id)!.position;
    expect(at("fetch").y).toBeGreaterThanOrEqual(at("trigger").y + NODE_H);
    expect(at("each").y).toBeGreaterThanOrEqual(at("fetch").y + NODE_H);
    expect(at("review").y).toBeGreaterThanOrEqual(at("dev").y + NODE_H);
    // The report sits below the whole group.
    const group = nodes.find((n) => n.id === "each")!;
    expect(at("report").y).toBeGreaterThanOrEqual(at("each").y + (group.height ?? 0));
    // Children are inside the group.
    expect(at("dev").x).toBeGreaterThanOrEqual(0);
    expect(at("dev").y).toBeGreaterThanOrEqual(0);
  });

  it("saves moved positions to node.ui and a new parent to node.parent", () => {
    const w = ticketWorkflow();
    const { nodes, edges } = toFlow(w);
    const moved = nodes.map((n) =>
      n.id === "fetch" ? { ...n, position: { x: 40, y: 120 } } : n,
    );
    expect(fromFlow(w, moved, edges).nodes.find((n) => n.id === "fetch")?.ui).toEqual({
      x: 40,
      y: 120,
    });
    const detached = nodes.map((n) =>
      n.id === "dev" ? { ...n, parentId: undefined, extent: undefined } : n,
    );
    const out = fromFlow(w, detached, edges).nodes.find((n) => n.id === "dev");
    expect(out).toBeDefined();
    expect("parent" in out!).toBe(false);
  });

  it("keeps the base order and appends new nodes", () => {
    const w = ticketWorkflow();
    const { nodes, edges } = toFlow(w);
    const added = newNode("command", w);
    const extra: FlowNode = {
      id: added.id,
      type: "board",
      position: { x: 5, y: 6 },
      data: { node: added },
    };
    const out = fromFlow(w, [extra, ...nodes], edges);
    expect(out.nodes.map((n) => n.id)).toEqual([...w.nodes.map((n) => n.id), added.id]);
    expect(out.nodes.at(-1)?.ui).toEqual({ x: 5, y: 6 });
  });
});

describe("connectionKind", () => {
  const w = ticketWorkflow();

  it("returns loop for review->dev", () => {
    const open = ticketWorkflow({
      edges: ticketWorkflow().edges.filter((e) => !e.loop),
    });
    expect(connectionKind(open, "review", "dev")).toBe("loop");
  });

  it("is invalid for a second loop out of the same node", () => {
    expect(connectionKind(w, "review", "dev")).toBe("invalid");
  });

  it("is invalid for an edge into a trigger", () => {
    expect(connectionKind(w, "fetch", "trigger")).toBe("invalid");
  });

  it("is invalid out of a report node and into itself", () => {
    expect(connectionKind(w, "report", "fetch")).toBe("invalid");
    expect(connectionKind(w, "fetch", "fetch")).toBe("invalid");
  });

  it("is invalid for an edge that already exists", () => {
    expect(connectionKind(w, "trigger", "fetch")).toBe("invalid");
  });

  it("is invalid across a For each boundary", () => {
    expect(connectionKind(w, "fetch", "dev")).toBe("invalid");
    expect(connectionKind(w, "review", "report")).toBe("invalid");
  });

  it("connects forward in a branch as a plain edge", () => {
    const wide = ticketWorkflow({
      nodes: [...ticketWorkflow().nodes, newNode("command", ticketWorkflow())],
    });
    const extra = wide.nodes.at(-1)!;
    expect(connectionKind(wide, "fetch", extra.id)).toBe("edge");
  });

  it("only lets a session, command or Ask Mono node start a loop", () => {
    const wide: Workflow = {
      ...ticketWorkflow(),
      nodes: [
        ...ticketWorkflow().nodes,
        { id: "gate", type: "if", condition: "{{dev.summary}} == 'x'", parent: "each", ui: { x: 16, y: 200 } },
      ],
      edges: [
        ...ticketWorkflow().edges.filter((e) => !e.loop),
        { from: "review", to: "gate" },
      ],
    };
    expect(connectionKind(wide, "gate", "dev")).toBe("invalid");
    expect(connectionKind(wide, "gate", "review")).toBe("invalid");
  });

  it("treats a loop target that is not an ancestor as invalid", () => {
    const wide = ticketWorkflow({
      nodes: [
        ...ticketWorkflow().nodes,
        { id: "other", type: "command", project: "{{inputs.project}}", command: "ls", parent: "each", ui: { x: 16, y: 220 } },
      ],
    });
    // Nothing links "other" to "dev", so going back to it is not a loop.
    expect(connectionKind(wide, "dev", "other")).toBe("edge");
    expect(connectionKind(wide, "other", "dev")).toBe("edge");
  });
});

describe("newNode", () => {
  it("ids are unique", () => {
    let w = ticketWorkflow();
    const seen = new Set(w.nodes.map((n) => n.id));
    for (const type of ["session", "session", "command", "askMono", "if", "forEach", "session"] as const) {
      const node = newNode(type, w);
      expect(seen.has(node.id)).toBe(false);
      expect(node.id).toMatch(/^[a-z][a-z0-9_]{0,39}$/);
      seen.add(node.id);
      w = { ...w, nodes: [...w.nodes, node] };
    }
  });

  it("gives each type sensible defaults", () => {
    const w = workflow({ inputs: { repo: { type: "project" } } });
    expect(newNode("session", w)).toMatchObject({
      type: "session",
      project: "{{inputs.repo}}",
      checkout: "root",
      prompt: "",
      output: {},
    });
    expect(newNode("command", w)).toMatchObject({ type: "command", project: "{{inputs.repo}}", command: "" });
    expect(newNode("askMono", w)).toMatchObject({ type: "askMono", answers: ["yes", "no"] });
    expect(newNode("if", w)).toMatchObject({ type: "if", decide: "mono" });
    expect(newNode("forEach", w)).toMatchObject({ type: "forEach", as: "item", maxParallel: 3 });
    expect(newNode("schedule", w)).toMatchObject({ type: "schedule", schedule: { scheduleKind: "weekdays" } });
  });

  it("leaves the project empty when no project input exists yet", () => {
    expect(newNode("session", workflow())).toMatchObject({ project: "" });
  });
});

describe("autoPlace", () => {
  it("puts a node below the lowest one, at the same x", () => {
    const w = ticketWorkflow();
    const place = autoPlace(w);
    // Top-level nodes: trigger 0, fetch 90, each 190 (a group), report 480.
    expect(place.x).toBe(0);
    expect(place.y).toBeGreaterThanOrEqual(480 + NODE_H);
  });

  it("works inside a group, relative to it", () => {
    const w = ticketWorkflow();
    const place = autoPlace(w, "each");
    expect(place.x).toBe(16);
    expect(place.y).toBeGreaterThanOrEqual(130 + NODE_H);
  });

  it("starts at the origin on an empty board", () => {
    expect(autoPlace(workflow({ nodes: [], edges: [] }))).toEqual({ x: 0, y: 0 });
  });
});

describe("groups", () => {
  it("fitGroups wraps the children with room to add another", () => {
    const { nodes } = toFlow(ticketWorkflow());
    const fitted = fitGroups(nodes);
    const group = fitted.find((n) => n.id === "each")!;
    const review = fitted.find((n) => n.id === "review")!;
    expect(group.width).toBeGreaterThanOrEqual(review.position.x + NODE_W);
    expect(group.height).toBeGreaterThan(review.position.y + NODE_H);
  });

  it("settleDrop attaches a node dropped inside a group and converts its position", () => {
    const { nodes } = toFlow(ticketWorkflow());
    const group = nodes.find((n) => n.id === "each")!;
    const dropped = nodes.map((n) =>
      n.id === "fetch"
        ? { ...n, position: { x: group.position.x + 30, y: group.position.y + 60 } }
        : n,
    );
    const after = settleDrop(dropped, "fetch");
    const fetch = after.find((n) => n.id === "fetch")!;
    expect(fetch).toMatchObject({ parentId: "each", extent: "parent" });
    expect(fetch.position).toEqual({ x: 30, y: 60 });
  });

  it("settleDrop detaches a child dragged out of its group", () => {
    const { nodes } = toFlow(ticketWorkflow());
    const dropped = nodes.map((n) =>
      n.id === "dev" ? { ...n, position: { x: 600, y: 40 } } : n,
    );
    const after = settleDrop(dropped, "dev");
    const dev = after.find((n) => n.id === "dev")!;
    expect(dev.parentId).toBeUndefined();
    expect(dev.extent).toBeUndefined();
    const group = nodes.find((n) => n.id === "each")!;
    expect(dev.position).toEqual({ x: group.position.x + 600, y: group.position.y + 40 });
  });

  it("settleDrop keeps a child inside its group and clamps it", () => {
    const { nodes } = toFlow(ticketWorkflow());
    const dropped = nodes.map((n) =>
      n.id === "dev" ? { ...n, position: { x: -5, y: 12 } } : n,
    );
    const dev = settleDrop(dropped, "dev").find((n) => n.id === "dev")!;
    expect(dev.parentId).toBe("each");
    expect(dev.position.x).toBeGreaterThanOrEqual(0);
    expect(dev.position.y).toBeGreaterThan(0);
  });

  it("settleDrop never nests a group and never puts the report in one", () => {
    const w = ticketWorkflow();
    const { nodes } = toFlow(w);
    const group = nodes.find((n) => n.id === "each")!;
    const inside = { x: group.position.x + 20, y: group.position.y + 60 };
    const report = settleDrop(
      nodes.map((n) => (n.id === "report" ? { ...n, position: inside } : n)),
      "report",
    ).find((n) => n.id === "report")!;
    expect(report.parentId).toBeUndefined();
    const second = { ...nodes.find((n) => n.id === "each")!, id: "each2" };
    const nested = settleDrop([...nodes, { ...second, position: inside }], "each2").find(
      (n) => n.id === "each2",
    )!;
    expect(nested.parentId).toBeUndefined();
  });

  it("pushBelow moves what is under a group that grew", () => {
    const { nodes } = toFlow(ticketWorkflow());
    const taller = nodes.map((n) => (n.id === "each" ? { ...n, height: (n.height ?? 0) + 60 } : n));
    const out = pushBelow(nodes, taller);
    expect(out.find((n) => n.id === "report")!.position.y).toBe(480 + 60);
    // Children, and nodes above the group, stay put; a group that did not grow moves nothing.
    expect(out.find((n) => n.id === "dev")!.position).toEqual(nodes.find((n) => n.id === "dev")!.position);
    expect(out.find((n) => n.id === "fetch")!.position.y).toBe(90);
    expect(pushBelow(nodes, nodes)).toEqual(nodes);
  });

  it("dropCrossingEdges removes the wires a node can no longer keep after changing groups", () => {
    const { nodes, edges } = toFlow(ticketWorkflow());
    const group = nodes.find((n) => n.id === "each")!;
    const moved = settleDrop(
      nodes.map((n) =>
        n.id === "fetch"
          ? { ...n, position: { x: group.position.x + 30, y: group.position.y + 60 } }
          : n,
      ),
      "fetch",
    );
    const kept = dropCrossingEdges(moved, edges, ["fetch"]);
    // trigger -> fetch and fetch -> each now cross the frame; the rest stay.
    expect(kept.map((e) => e.id)).toEqual(
      edges.filter((e) => e.source !== "fetch" && e.target !== "fetch").map((e) => e.id),
    );
    // A node that did not change scope keeps all of its wires.
    expect(dropCrossingEdges(nodes, edges, ["dev"])).toEqual(edges);
  });

  it("dropNodes removes edges and lifts the children of a removed group", () => {
    const { nodes, edges } = toFlow(ticketWorkflow());
    const out = dropNodes(nodes, edges, ["each"]);
    expect(out.nodes.find((n) => n.id === "each")).toBeUndefined();
    const dev = out.nodes.find((n) => n.id === "dev")!;
    expect(dev.parentId).toBeUndefined();
    const group = nodes.find((n) => n.id === "each")!;
    expect(dev.position).toEqual({
      x: group.position.x + 16,
      y: group.position.y + 40,
    });
    expect(out.edges.some((e) => e.source === "each" || e.target === "each")).toBe(false);
    expect(out.edges.some((e) => e.source === "dev" && e.target === "review")).toBe(true);
  });
});

describe("projectInputFor", () => {
  it("creates a project input named project for the first project", () => {
    const w = workflow();
    const out = projectInputFor(w, "/work/api", "api");
    expect(out.ref).toBe("{{inputs.project}}");
    expect(out.inputs.project).toEqual({ type: "project" });
    expect(out.inputValues.project).toBe("/work/api");
  });

  it("reuses the input that already holds that project", () => {
    const w = ticketWorkflow();
    const out = projectInputFor(w, "/work/api", "api");
    expect(out.ref).toBe("{{inputs.project}}");
    expect(Object.keys(out.inputs)).toEqual(Object.keys(w.inputs));
  });

  it("names another project after it and keeps the first", () => {
    const w = ticketWorkflow();
    const out = projectInputFor(w, "/work/web app", "web app");
    expect(out.ref).toBe("{{inputs.web_app}}");
    expect(out.inputs.web_app).toEqual({ type: "project" });
    expect(out.inputValues).toMatchObject({ project: "/work/api", web_app: "/work/web app" });
  });

  it("never reuses a name that is taken by another kind of input", () => {
    const w = workflow({ inputs: { project: { type: "string" } }, inputValues: { project: "x" } });
    const out = projectInputFor(w, "/work/api", "api");
    expect(out.ref).toBe("{{inputs.api}}");
  });
});

describe("keyboard delete", () => {
  it("removes a For each and its wires but keeps its children with theirs", () => {
    const { nodes, edges } = toFlow(ticketWorkflow());
    // What React Flow hands over for a selected group: the group, its children, and every
    // edge touching any of them.
    const group = { ...nodes.find((n) => n.id === "each")!, selected: true };
    const children = nodes.filter((n) => n.parentId === "each");
    const listed = [group, ...children];
    const out = deletion(listed, edges);
    expect(out.nodes.map((n) => n.id)).toEqual(["each"]);
    expect(out.edges.map((e) => e.id)).toEqual(["fetch->each", "each->report"]);
  });

  it("removes children that were selected themselves, and edges that were selected", () => {
    const { nodes, edges } = toFlow(ticketWorkflow());
    const dev = { ...nodes.find((n) => n.id === "dev")!, selected: true };
    const wire = { ...edges.find((e) => e.id === "trigger->fetch")!, selected: true };
    const out = deletion([dev], [...edges.filter((e) => e.id !== wire.id), wire]);
    expect(out.nodes.map((n) => n.id)).toEqual(["dev"]);
    expect(out.edges.map((e) => e.id).sort()).toEqual(["dev->review", "review->dev~loop", "trigger->fetch"].sort());
  });
});

describe("If handles", () => {
  const w = workflow({
    nodes: [
      { id: "trigger", type: "manual" },
      { id: "check", type: "if", decide: "mono" },
      { id: "report", type: "report" },
    ],
    edges: [
      { from: "trigger", to: "check" },
      { from: "check", to: "report", handle: "true" },
    ],
  });

  it("lets true and false lead to the same node, but not the same handle twice", () => {
    expect(connectionKind(w, "check", "report", "false")).toBe("edge");
    expect(connectionKind(w, "check", "report", "true")).toBe("invalid");
  });
});

describe("definitionKey", () => {
  it("treats a whitespace-only description as none", () => {
    const w = ticketWorkflow();
    const { description: _d, ...bare } = w;
    expect(definitionKey({ ...w, description: "   " })).toBe(definitionKey(bare));
  });
});

describe("projectInputFor and inputs with no value", () => {
  it("fills a declared project input that has no value yet", () => {
    const w = workflow({ inputs: { repo: { type: "project" } } });
    const out = projectInputFor(w, "/work/api", "api");
    expect(out.ref).toBe("{{inputs.repo}}");
    expect(Object.keys(out.inputs)).toEqual(["repo"]);
    expect(out.inputValues.repo).toBe("/work/api");
  });

  it("does not bind a step to a valueless input another step uses", () => {
    const w = workflow({
      inputs: { api: { type: "project" } },
      nodes: [
        { id: "trigger", type: "manual" },
        { id: "a", type: "command", project: "{{inputs.api}}", command: "ls" },
        { id: "report", type: "report" },
      ],
    });
    const out = projectInputFor(w, "/work/web", "web");
    expect(out.ref).toBe("{{inputs.project}}");
    expect(out.inputs.api).toEqual({ type: "project" });
    expect(out.inputValues).toEqual({ project: "/work/web" });
    // A loop condition that mentions the input counts as using it too.
    const looped = workflow({
      inputs: { api: { type: "project" } },
      edges: [{ from: "trigger", to: "report", loop: { maxRounds: 2, decide: "{{inputs.api}} == 1" } }],
    });
    expect(projectInputFor(looped, "/work/web", "web").ref).toBe("{{inputs.project}}");
  });

  it("reuses the step's own valueless input", () => {
    const w = workflow({
      inputs: { api: { type: "project" }, other: { type: "project" } },
      nodes: [
        { id: "trigger", type: "manual" },
        { id: "a", type: "command", project: "{{inputs.api}}", command: "ls" },
        { id: "report", type: "report" },
      ],
    });
    const out = projectInputFor(w, "/work/web", "web", "api");
    expect(out.ref).toBe("{{inputs.api}}");
    expect(out.inputValues).toEqual({ api: "/work/web" });
  });

  it("does not mistake api_2 for api when looking for use", () => {
    const w = workflow({
      inputs: { api: { type: "project" }, api_2: { type: "project" } },
      nodes: [
        { id: "trigger", type: "manual" },
        { id: "a", type: "command", project: "{{inputs.api_2}}", command: "ls" },
        { id: "report", type: "report" },
      ],
    });
    expect(projectInputFor(w, "/work/web", "web").ref).toBe("{{inputs.api}}");
  });

  it("prefers the input the step already uses", () => {
    const w = workflow({ inputs: { a: { type: "project" }, b: { type: "project" } } });
    expect(projectInputFor(w, "/work/api", "api", "b").ref).toBe("{{inputs.b}}");
  });
});
