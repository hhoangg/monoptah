import type { Edge, Node } from "@xyflow/react";
import {
  isTrigger,
  newWorkflowId,
  type LoopSpec,
  type NodeId,
  type NodeType,
  type Workflow,
  type WorkflowEdge,
  type WorkflowInput,
  type WorkflowNode,
} from "../../model/workflow";
import type { WorkflowIssue } from "../../model/workflowParse";

/**
 * The board's pure side: a workflow as React Flow nodes and edges and back, the rules for what
 * may be connected, and where things sit. A For each is a group node; its children carry
 * `parentId` and a position relative to it, which is what `node.ui` stores for them too.
 */

/** A node card is this wide and about this tall; layout counts on it. */
export const NODE_W = 232;
export const NODE_H = 48;
/** Space between a node and the next one down. */
export const GAP_Y = 36;
const GAP_X = 28;
/** Room around the children of a group, and above them for its title. */
export const GROUP_PAD = 16;
export const GROUP_HEADER = 32;
/** The loop edge runs down the right of a group, so that side is wider. */
const GROUP_RIGHT = 52;
/** Space left under the last child, so another can be dropped in. */
const GROUP_SLACK = 44;

/** The height of a For each with nothing in it yet. */
export const EMPTY_GROUP_H = GROUP_HEADER + NODE_H + GROUP_PAD + GROUP_SLACK;

export type BoardNodeData = {
  node: WorkflowNode;
  /** What `validateWorkflow` found, set when drawn. */
  issues?: string[];
  /** Whether a loop edge enters or leaves the node, set when drawn. */
  loopIn?: boolean;
  loopOut?: boolean;
  /** For a For each: nothing is in it yet. */
  empty?: boolean;
};
export type FlowNode = Node<BoardNodeData, "board" | "group">;
export type FlowEdge = Edge<{ loop?: LoopSpec }>;

type Point = { x: number; y: number };

/** Handle ids: the two outs of an If node, and the side handles a loop edge uses. */
export const LOOP_OUT = "loop-out";
export const LOOP_IN = "loop-in";

/** Whether an object has its own key `key` (not one from its prototype). */
export const has = (object: object, key: string): boolean =>
  Object.prototype.hasOwnProperty.call(object, key);

const LOOP_SOURCES: readonly NodeType[] = ["session", "command", "askMono"];

export function isGroup(node: WorkflowNode): boolean {
  return node.type === "forEach";
}

/** Whether a node may sit inside a For each. */
export function groupable(node: WorkflowNode): boolean {
  return node.type !== "forEach" && node.type !== "report" && !isTrigger(node);
}

// --- layout ---------------------------------------------------------------------------------

function groupSize(children: readonly Point[]): { width: number; height: number } {
  if (children.length === 0) {
    return {
      width: NODE_W + GROUP_PAD + GROUP_RIGHT,
      height: EMPTY_GROUP_H,
    };
  }
  const right = Math.max(...children.map((p) => p.x + NODE_W));
  const bottom = Math.max(...children.map((p) => p.y + NODE_H));
  return {
    width: Math.max(NODE_W + GROUP_PAD + GROUP_RIGHT, right + GROUP_RIGHT),
    height: bottom + GROUP_PAD + GROUP_SLACK,
  };
}

/** The For each a node sits in, if there is one that exists. */
function groupOf(w: Workflow, node: WorkflowNode): NodeId | undefined {
  if (node.parent === undefined || node.type === "forEach") return undefined;
  return w.nodes.some((n) => n.id === node.parent && n.type === "forEach")
    ? node.parent
    : undefined;
}

/** Positions for nodes of one scope that have none: layers by depth, one row per layer. */
function layoutScope(
  w: Workflow,
  members: readonly WorkflowNode[],
  origin: Point,
  heights: ReadonlyMap<NodeId, number>,
  widths: ReadonlyMap<NodeId, number>,
): Map<NodeId, Point> {
  const ids = new Set(members.map((n) => n.id));
  const edges = w.edges.filter((e) => !e.loop && ids.has(e.from) && ids.has(e.to));
  const depth = new Map<NodeId, number>(members.map((n) => [n.id, 0]));
  // A cycle without a loop edge is a validation problem; the pass count keeps it finite.
  for (let pass = 0; pass < members.length; pass++) {
    let moved = false;
    for (const e of edges) {
      const next = (depth.get(e.from) ?? 0) + 1;
      if (next > (depth.get(e.to) ?? 0) && next < members.length) {
        depth.set(e.to, next);
        moved = true;
      }
    }
    if (!moved) break;
  }
  const layers: WorkflowNode[][] = [];
  for (const n of members) (layers[depth.get(n.id) ?? 0] ??= []).push(n);
  const out = new Map<NodeId, Point>();
  let y = origin.y;
  for (const layer of layers) {
    if (!layer) continue;
    let x = origin.x;
    let tallest = NODE_H;
    for (const n of layer) {
      out.set(n.id, { x, y });
      x += (widths.get(n.id) ?? NODE_W) + GAP_X;
      tallest = Math.max(tallest, heights.get(n.id) ?? NODE_H);
    }
    y += tallest + GAP_Y;
  }
  return out;
}

type Layout = {
  at: Map<NodeId, Point>;
  size: Map<NodeId, { width: number; height: number }>;
};

/** Where every node sits: its saved position, else one made by laying its scope out. */
function layoutOf(w: Workflow): Layout {
  const at = new Map<NodeId, Point>();
  const size = new Map<NodeId, { width: number; height: number }>();
  const groups = w.nodes.filter(isGroup);
  for (const group of groups) {
    const children = w.nodes.filter((n) => groupOf(w, n) === group.id);
    const auto = layoutScope(w, children, { x: GROUP_PAD, y: GROUP_HEADER }, new Map(), new Map());
    for (const child of children) at.set(child.id, child.ui ?? auto.get(child.id) ?? { x: GROUP_PAD, y: GROUP_HEADER });
    size.set(group.id, groupSize(children.map((c) => at.get(c.id) as Point)));
  }
  const top = w.nodes.filter((n) => groupOf(w, n) === undefined);
  const heights = new Map([...size].map(([id, s]) => [id, s.height]));
  const widths = new Map([...size].map(([id, s]) => [id, s.width]));
  const auto = layoutScope(w, top, { x: 0, y: 0 }, heights, widths);
  for (const node of top) {
    const p = auto.get(node.id) ?? { x: 0, y: 0 };
    // A group sits a padding to the left, so the nodes inside line up with the column.
    at.set(node.id, node.ui ?? (isGroup(node) ? { x: p.x - GROUP_PAD, y: p.y } : p));
  }
  return { at, size };
}

/** Where a new node goes: under the lowest one of its scope, in the same column. */
export function autoPlace(w: Workflow, parent?: NodeId): { x: number; y: number } {
  const { at, size } = layoutOf(w);
  const inScope = w.nodes.filter((n) => groupOf(w, n) === parent);
  if (inScope.length === 0) {
    return parent === undefined ? { x: 0, y: 0 } : { x: GROUP_PAD, y: GROUP_HEADER };
  }
  let lowest = inScope[0];
  for (const n of inScope) {
    if ((at.get(n.id)?.y ?? 0) >= (at.get(lowest.id)?.y ?? 0)) lowest = n;
  }
  const p = at.get(lowest.id) as Point;
  return {
    x: isGroup(lowest) && parent === undefined ? p.x + GROUP_PAD : p.x,
    y: p.y + (size.get(lowest.id)?.height ?? NODE_H) + GAP_Y,
  };
}

// --- the workflow as a flow --------------------------------------------------------------------

function edgeId(e: WorkflowEdge): string {
  return `${e.from}${e.handle ? `.${e.handle}` : ""}->${e.to}${e.loop ? "~loop" : ""}`;
}

export function makeEdge(
  from: NodeId,
  to: NodeId,
  options: { handle?: "true" | "false"; loop?: LoopSpec } = {},
): FlowEdge {
  const { handle, loop } = options;
  const id = edgeId({ from, to, ...(handle ? { handle } : {}), ...(loop ? { loop } : {}) });
  return loop
    ? {
        id,
        type: "loop",
        source: from,
        target: to,
        sourceHandle: LOOP_OUT,
        targetHandle: LOOP_IN,
        data: { loop },
      }
    : {
        id,
        type: "smoothstep",
        source: from,
        target: to,
        ...(handle ? { sourceHandle: handle } : {}),
        data: {},
      };
}

/** React Flow wants a group before the nodes inside it. */
function parentsFirst(nodes: FlowNode[]): FlowNode[] {
  return [...nodes.filter((n) => n.type === "group"), ...nodes.filter((n) => n.type !== "group")];
}

export function toFlow(w: Workflow): { nodes: FlowNode[]; edges: FlowEdge[] } {
  const { at, size } = layoutOf(w);
  const nodes = w.nodes.map((node): FlowNode => {
    const position = at.get(node.id) ?? { x: 0, y: 0 };
    if (isGroup(node)) {
      const { width, height } = size.get(node.id) ?? groupSize([]);
      return { id: node.id, type: "group", position, width, height, data: { node } };
    }
    const parentId = groupOf(w, node);
    return {
      id: node.id,
      type: "board",
      position,
      width: NODE_W,
      data: { node },
      ...(parentId ? { parentId, extent: "parent" as const } : {}),
    };
  });
  const seen = new Set<string>();
  const edges = w.edges.map((e) => {
    const edge = makeEdge(e.from, e.to, { handle: e.handle, loop: e.loop });
    let id = edge.id;
    for (let n = 2; seen.has(id); n++) id = `${edge.id}#${n}`;
    seen.add(id);
    return { ...edge, id };
  });
  return { nodes: parentsFirst(nodes), edges };
}

const round = (n: number) => Math.round(n * 10) / 10;

export function fromFlow(base: Workflow, nodes: FlowNode[], edges: FlowEdge[]): Workflow {
  const flowById = new Map(nodes.map((n) => [n.id, n]));
  const order = [
    ...base.nodes.map((n) => n.id).filter((id) => flowById.has(id)),
    ...nodes.map((n) => n.id).filter((id) => !base.nodes.some((b) => b.id === id)),
  ];
  const out = order.map((id): WorkflowNode => {
    const flow = flowById.get(id) as FlowNode;
    const { parent: _parent, ui: _ui, ...rest } = flow.data.node;
    return {
      ...rest,
      ...(flow.parentId ? { parent: flow.parentId } : {}),
      ui: { x: round(flow.position.x), y: round(flow.position.y) },
    } as WorkflowNode;
  });
  const outEdges = edges.map((e): WorkflowEdge => {
    const loop = e.data?.loop;
    const handle =
      e.sourceHandle === "true" || e.sourceHandle === "false" ? e.sourceHandle : undefined;
    return {
      from: e.source,
      to: e.target,
      ...(handle && !loop ? { handle } : {}),
      ...(loop ? { loop } : {}),
    };
  });
  return { ...base, nodes: out, edges: outEdges };
}

// --- what may be connected -----------------------------------------------------------------------

/** Whether `to` can be reached from `from` along ordinary (non-loop) edges. */
function reaches(w: Workflow, from: NodeId, to: NodeId): boolean {
  const seen = new Set<NodeId>([from]);
  const queue = [from];
  while (queue.length) {
    const id = queue.pop() as NodeId;
    for (const e of w.edges) {
      if (e.loop || e.from !== id || seen.has(e.to)) continue;
      if (e.to === to) return true;
      seen.add(e.to);
      queue.push(e.to);
    }
  }
  return false;
}

/**
 * What dragging a connection from `from` to `to` makes. A loop is a connection back to an
 * ancestor in the same branch, allowed from a session, command or Ask Mono node that has no
 * loop yet; anything else that would break the graph's rules is invalid.
 */
export function connectionKind(
  w: Workflow,
  from: NodeId,
  to: NodeId,
  /** The If handle the connection leaves from; an If node may send both to one node. */
  handle?: "true" | "false",
): "edge" | "loop" | "invalid" {
  const a = w.nodes.find((n) => n.id === from);
  const b = w.nodes.find((n) => n.id === to);
  if (!a || !b || from === to) return "invalid";
  if (isTrigger(b) || a.type === "report") return "invalid";
  if (a.parent !== b.parent) return "invalid";
  if (w.edges.some((e) => !e.loop && e.from === from && e.to === to && e.handle === handle))
    return "invalid";
  if (reaches(w, to, from)) {
    if (!LOOP_SOURCES.includes(a.type)) return "invalid";
    if (w.edges.some((e) => e.loop && e.from === from)) return "invalid";
    return "loop";
  }
  return "edge";
}

// --- new nodes -------------------------------------------------------------------------------------

const ID_BASE: Record<NodeType, string> = {
  schedule: "trigger",
  manual: "trigger",
  session: "session",
  command: "command",
  askMono: "ask",
  if: "check",
  forEach: "each",
  report: "report",
};

function uniqueId(w: Workflow, base: string): NodeId {
  const taken = new Set(w.nodes.map((n) => n.id));
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base}_${n}`)) return `${base}_${n}`;
}

/** The first `project` input of the workflow as a node's `project`, or "" when it has none. */
function defaultProject(w: Workflow): string {
  const name = Object.keys(w.inputs).find((key) => w.inputs[key].type === "project");
  return name ? `{{inputs.${name}}}` : "";
}

export function newNode(type: NodeType, w: Workflow): WorkflowNode {
  const id = uniqueId(w, ID_BASE[type]);
  switch (type) {
    case "schedule":
      return {
        id,
        type,
        schedule: { scheduleKind: "weekdays", time: "09:00", minute: 0, dayOfWeek: 1 },
      };
    case "manual":
      return { id, type };
    case "session":
      return {
        id,
        type,
        project: defaultProject(w),
        prompt: "",
        checkout: "root",
        output: {},
      };
    case "command":
      return { id, type, project: defaultProject(w), command: "" };
    case "askMono":
      return { id, type, question: "", answers: ["yes", "no"] };
    case "if":
      return { id, type, decide: "mono" };
    case "forEach":
      return { id, type, in: "", as: "item", maxParallel: 3 };
    case "report":
      return { id, type };
  }
}

/** "web app" -> "web_app": something that can be an input name. */
function inputName(label: string): string {
  const name = label
    .replace(/[^A-Za-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .toLowerCase();
  return /^[a-z_]/.test(name) ? name : name ? `p_${name}` : "";
}

/**
 * The `project` input that stands for a project: the one already holding it, or a new one
 * (named "project" the first time, after the project's name later). A node's `project` is
 * then `{{inputs.<name>}}`, so the definition never holds a path.
 */
export function projectInputFor(
  w: Workflow,
  path: string,
  label: string,
  /** The input the step uses now, which takes the project first when it has no value. */
  current?: string,
): {
  ref: string;
  inputs: Record<string, WorkflowInput>;
  inputValues: Workflow["inputValues"];
} {
  const reuse = (name: string, value: string) => ({
    ref: `{{inputs.${name}}}`,
    inputs: w.inputs,
    inputValues: w.inputValues[name] === value ? w.inputValues : { ...w.inputValues, [name]: value },
  });
  const isProject = (key: string) => has(w.inputs, key) && w.inputs[key].type === "project";
  const unset = (key: string) => isProject(key) && !w.inputValues[key];
  const held = Object.keys(w.inputs).find((key) => isProject(key) && w.inputValues[key] === path);
  if (held) return reuse(held, path);
  // An input that is declared but has no value is there to be filled, when it is the step's
  // own or nothing else uses it: binding another step to it would move that step's project.
  const text = JSON.stringify({ nodes: w.nodes, edges: w.edges });
  const used = (key: string) => new RegExp(`inputs\\.${key}(?![A-Za-z0-9_])`).test(text);
  const empty = current && unset(current) ? current : Object.keys(w.inputs).find((k) => unset(k) && !used(k));
  if (empty) return reuse(empty, path);
  const base = has(w.inputs, "project") ? inputName(label) || "project" : "project";
  let name = base;
  for (let n = 2; has(w.inputs, name); n++) name = `${base}_${n}`;
  return {
    ref: `{{inputs.${name}}}`,
    inputs: { ...w.inputs, [name]: { type: "project" } },
    inputValues: { ...w.inputValues, [name]: path },
  };
}

// --- groups ------------------------------------------------------------------------------------------

/** Every group sized to wrap its children. */
export function fitGroups(nodes: FlowNode[]): FlowNode[] {
  return nodes.map((n) => {
    if (n.type !== "group") return n;
    const { width, height } = groupSize(
      nodes.filter((c) => c.parentId === n.id).map((c) => c.position),
    );
    return n.width === width && n.height === height ? n : { ...n, width, height };
  });
}

/**
 * `after` (a refit of `before`) with the nodes below a group that grew moved down by as much,
 * so growing a For each does not run it into what follows.
 */
export function pushBelow(before: FlowNode[], after: FlowNode[]): FlowNode[] {
  let out = after;
  for (const g of before) {
    if (g.type !== "group") continue;
    const grown = (after.find((n) => n.id === g.id)?.height ?? 0) - (g.height ?? 0);
    if (grown <= 0) continue;
    const edge = g.position.y + (g.height ?? 0);
    out = out.map((n) =>
      !n.parentId && n.id !== g.id && n.position.y >= edge
        ? { ...n, position: { x: n.position.x, y: n.position.y + grown } }
        : n,
    );
  }
  return out;
}

function absolute(nodes: FlowNode[], node: FlowNode): Point {
  const parent = node.parentId ? nodes.find((n) => n.id === node.parentId) : undefined;
  return parent
    ? { x: parent.position.x + node.position.x, y: parent.position.y + node.position.y }
    : node.position;
}

/**
 * After a node was dragged: it joins the For each its centre is over, or leaves the one it
 * was in when it is no longer over it. Positions are converted so it stays where it was
 * dropped; a child is kept inside its group's padding, and the groups are refitted.
 */
export function settleDrop(nodes: FlowNode[], id: NodeId): FlowNode[] {
  const node = nodes.find((n) => n.id === id);
  if (!node) return nodes;
  const abs = absolute(nodes, node);
  const centre = { x: abs.x + NODE_W / 2, y: abs.y + NODE_H / 2 };
  const target = groupable(node.data.node)
    ? nodes.find(
        (g) =>
          g.type === "group" &&
          g.id !== id &&
          centre.x >= g.position.x &&
          centre.x <= g.position.x + (g.width ?? 0) &&
          centre.y >= g.position.y &&
          centre.y <= g.position.y + (g.height ?? 0),
      )
    : undefined;
  const { parentId: _parent, extent: _extent, ...free } = node;
  const settled: FlowNode = target
    ? {
        ...free,
        parentId: target.id,
        extent: "parent",
        position: {
          x: Math.max(GROUP_PAD, abs.x - target.position.x),
          y: Math.max(GROUP_HEADER, abs.y - target.position.y),
        },
      }
    : { ...free, position: abs };
  const placed = parentsFirst(nodes.map((n) => (n.id === id ? settled : n)));
  return pushBelow(placed, fitGroups(placed));
}

/**
 * The edges without the ones that touch `ids` and now join nodes of different scopes. A node
 * that joined or left a For each can no longer be wired to nodes on the other side of its
 * frame, and the engine only follows edges between nodes with the same parent.
 */
export function dropCrossingEdges(
  nodes: readonly FlowNode[],
  edges: FlowEdge[],
  ids: readonly NodeId[],
): FlowEdge[] {
  const parentOf = new Map(nodes.map((n) => [n.id, n.parentId]));
  return edges.filter(
    (e) =>
      !(ids.includes(e.source) || ids.includes(e.target)) ||
      parentOf.get(e.source) === parentOf.get(e.target),
  );
}

function withoutRefs(node: WorkflowNode, gone: ReadonlySet<NodeId>): WorkflowNode {
  if (node.type === "session") {
    const next = { ...node };
    if (next.nestUnder && gone.has(next.nestUnder)) delete next.nestUnder;
    if (typeof next.checkout === "object" && gone.has(next.checkout.worktreeOf)) next.checkout = "root";
    return next;
  }
  if (node.type === "command" && typeof node.cwd === "object" && gone.has(node.cwd.worktreeOf)) {
    const { cwd: _cwd, ...rest } = node;
    return rest;
  }
  return node;
}

/**
 * The board without some nodes: their edges go, the children of a removed For each become
 * top-level where they were drawn, and nothing is left pointing at a removed session.
 */
export function dropNodes(
  nodes: FlowNode[],
  edges: FlowEdge[],
  ids: readonly NodeId[],
): { nodes: FlowNode[]; edges: FlowEdge[] } {
  const gone = new Set(ids);
  const kept = nodes
    .filter((n) => !gone.has(n.id))
    .map((n): FlowNode => {
      const lifted = n.parentId && gone.has(n.parentId);
      const { parentId: _p, extent: _e, ...free } = n;
      const base = lifted ? { ...free, position: absolute(nodes, n) } : n;
      return { ...base, data: { ...base.data, node: withoutRefs(base.data.node, gone) } };
    });
  return {
    nodes: fitGroups(kept),
    edges: edges.filter((e) => !gone.has(e.source) && !gone.has(e.target)),
  };
}

/**
 * What a keyboard delete removes. React Flow also lists the children of a deleted For each,
 * and every edge touching them; the children stay (the group's frame goes) and so do their
 * wires. Edges go when they touch a removed node or were selected themselves.
 */
export function deletion(
  nodes: FlowNode[],
  edges: FlowEdge[],
): { nodes: FlowNode[]; edges: FlowEdge[] } {
  const removed = nodes.filter(
    (n) => !n.parentId || n.selected || !nodes.some((p) => p.id === n.parentId),
  );
  const ids = new Set(removed.map((n) => n.id));
  return {
    nodes: removed,
    edges: edges.filter((e) => ids.has(e.source) || ids.has(e.target) || e.selected),
  };
}

/** The messages of the issues that belong to one node, by node id. */
export function issuesByNode(issues: readonly WorkflowIssue[]): Map<NodeId, string[]> {
  const out = new Map<NodeId, string[]>();
  for (const issue of issues) {
    if (issue.nodeId === undefined) continue;
    out.set(issue.nodeId, [...(out.get(issue.nodeId) ?? []), issue.message]);
  }
  return out;
}

// --- whole workflows ---------------------------------------------------------------------------------

/** A workflow to start from: a manual trigger into a Report node, paused. */
export function blankWorkflow(): Workflow {
  return {
    id: newWorkflowId(),
    name: "New workflow",
    inputs: {},
    inputValues: {},
    nodes: [
      { id: "trigger", type: "manual", title: "Manual trigger", ui: { x: 0, y: 0 } },
      { id: "report", type: "report", title: "Report to Mono", ui: { x: 0, y: NODE_H + GAP_Y } },
    ],
    edges: [{ from: "trigger", to: "report" }],
    enabled: false,
    createdAt: Date.now(),
  };
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, v]) => v !== undefined)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([k, v]) => [k, canonical(v)]),
    );
  }
  return value;
}

/**
 * What makes two versions of a workflow the same to the editor: its definition as the board
 * would draw it (so a workflow with no saved layout equals the same one after being laid out),
 * and not the fields the app manages, which change without the user editing anything.
 */
export function definitionKey(w: Workflow): string {
  const { nodes, edges } = toFlow(w);
  const flat = fromFlow(w, nodes, edges);
  return JSON.stringify(
    canonical({
      name: w.name,
      description: w.description?.trim() ? w.description : "",
      enabled: w.enabled,
      inputs: w.inputs,
      inputValues: Object.fromEntries(
        Object.entries(w.inputValues).filter(([name]) => has(w.inputs, name)),
      ),
      nodes: flat.nodes,
      edges: flat.edges,
    }),
  );
}
