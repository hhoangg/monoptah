import {
  archivedBranch,
  archivedItemsKey,
  parseBranch,
  type BranchKey,
  type LoopSpec,
  type NodeId,
  type NodeRun,
  type WorkflowEdge,
  type WorkflowNode,
  type WorkflowRun,
} from "../../model/workflow";
import { isAsking } from "../../model/workflowEngine";
import {
  GROUP_PAD,
  NODE_H,
  NODE_W,
  toFlow,
  type FlowEdge,
  type FlowNode,
} from "../board/boardModel";

/**
 * The pure side of the run graph: a run's definition snapshot as React Flow nodes and edges,
 * each carrying what the run did with it. A For each is drawn either as one lane per item
 * (side by side) or as one lane at a time; the lane's child nodes get ids that name their
 * branch, so the same definition node can appear in every lane.
 */

export type RunNodeStatus =
  | "done"
  | "running"
  | "waiting"
  | "failed"
  | "skipped"
  | "pending"
  | "stopped";
export type RunNodeState = {
  status: RunNodeStatus;
  /** `${nodeId}|${branch}`, the key `nodeRunsByKey` uses; selects a node. */
  nodeRunKey: string;
  round: number;
  /** The card's second line, e.g. "Done · 12s", "Running 3m", "Done · true", "Failed: <error>". */
  label: string;
};
export type RunEdgeState = "taken" | "active" | "idle";
export type RunView = "lanes" | "single";
export type GroupView = {
  groupId: NodeId;
  /** Rounds this For each expanded in, ascending; empty when it never expanded. */
  rounds: number[];
  /** The selected round (default the latest). */
  round: number;
  items: { index: number; title: string; status: RunNodeStatus }[];
  /** The selected item for the single view (default the first running one, else the first). */
  item: number;
  /** "2 done · 1 running · 1 failed", "No items", or the failure text. */
  summary: string;
};

/** Space above the lanes of a group, for lane titles or item tabs. */
export const LANE_HEADER = 28;
/** Space between two lanes; wide enough for the loop edge that runs down a lane's right. */
export const LANE_GAP = 52;

const LANES_MAX_ITEMS = 4;
const LANES_MAX_CHILDREN = 3;
const LABEL_MAX = 60;

export const nodeRunKey = (nodeId: NodeId, branch: BranchKey): string => `${nodeId}|${branch}`;

export function nodeRunsByKey(run: WorkflowRun): Map<string, NodeRun> {
  return new Map((run.nodes ?? []).map((n) => [nodeRunKey(n.nodeId, n.branch), n]));
}

function statusOf(r: NodeRun | undefined): RunNodeStatus {
  if (!r) return "pending";
  if (r.status === "waitingApproval") return "waiting";
  if (r.status === "running" && isAsking(r)) return "waiting";
  return r.status;
}

function span(ms: number): string {
  const seconds = Math.max(1, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return rest ? `${minutes}m ${rest}s` : `${minutes}m`;
}

function short(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > LABEL_MAX ? `${flat.slice(0, LABEL_MAX - 3)}...` : flat;
}

/** How long a node that has finished took, when it is long enough to say. */
function took(r: NodeRun | undefined): string[] {
  if (r?.startedAt === undefined || r.endedAt === undefined || r.endedAt - r.startedAt < 1000) return [];
  return [span(r.endedAt - r.startedAt)];
}

function labelOf(r: NodeRun | undefined, status: RunNodeStatus, now: number): string {
  const parts: string[] = [];
  switch (status) {
    case "done": {
      parts.push("Done");
      if (r?.decision) {
        const reason = r.decision.reason ? `: ${short(r.decision.reason)}` : "";
        parts.push(`${r.decision.value}${reason}`);
      }
      parts.push(...took(r));
      break;
    }
    case "running":
      parts.push(r?.startedAt !== undefined ? `Running ${span(now - r.startedAt)}` : "Running");
      break;
    case "waiting":
      parts.push(r && isAsking(r) ? "Asking the Mono" : "Waiting for approval");
      break;
    case "failed":
      parts.push(r?.error ? `Failed: ${short(r.error)}` : "Failed", ...took(r));
      break;
    case "skipped":
      parts.push("Skipped");
      break;
    case "stopped":
      parts.push("Stopped", ...took(r));
      break;
    case "pending":
      parts.push("Not started");
      break;
  }
  if (r && r.round > 1) parts.push(`round ${r.round}`);
  return parts.join(" · ");
}

function stateOf(
  byKey: Map<string, NodeRun>,
  nodeId: NodeId,
  branch: BranchKey,
  now: number,
): RunNodeState {
  const key = nodeRunKey(nodeId, branch);
  const r = byKey.get(key);
  const status = statusOf(r);
  return { status, nodeRunKey: key, round: r?.round ?? 1, label: labelOf(r, status, now) };
}

// --- For each: rounds, items, branches --------------------------------------------------------

type ForEach = Extract<WorkflowNode, { type: "forEach" }>;

const liveBranch = (groupId: NodeId, index: number): BranchKey => `${groupId}#${index}`;

/** The branch of one item in one round; `live` is the round the For each is in now. */
function branchOf(groupId: NodeId, index: number, round: number, live: number | undefined): BranchKey {
  const branch = liveBranch(groupId, index);
  return round === live ? branch : archivedBranch(branch, round);
}

type Rounds = {
  /** Ascending. */
  rounds: number[];
  /** The round the For each is expanded in now, when it is. */
  live?: number;
  /** How many items a round had. */
  count: (round: number) => number;
  /** The item list of a round, when the run kept it. */
  list: (round: number) => unknown[] | undefined;
};

function roundsOf(run: WorkflowRun, group: ForEach): Rounds {
  const items = run.items ?? {};
  const archived = new Set<number>();
  const widest = new Map<number, number>();
  for (const n of run.nodes ?? []) {
    const parts = parseBranch(n.branch);
    if (!parts || parts.groupId !== group.id) continue;
    const round = parts.round ?? 0;
    if (round > 0) archived.add(round);
    widest.set(round, Math.max(widest.get(round) ?? 0, parts.index + 1));
  }
  for (const key of Object.keys(items)) {
    // An item list the run kept without any branch left (hand-edited or trimmed data).
    const tail = key.startsWith(`${group.id}@r`) ? Number(key.slice(group.id.length + 2)) : NaN;
    if (Number.isInteger(tail) && tail > 0 && key === archivedItemsKey(group.id, tail)) archived.add(tail);
  }
  const liveItems = items[group.id];
  const own = (run.nodes ?? []).find((n) => n.nodeId === group.id && n.branch === "")?.round ?? 1;
  const lastArchived = Math.max(0, ...archived);
  let live: number | undefined;
  if (Array.isArray(liveItems)) live = Math.max(own, lastArchived + 1);
  // A loop sent the group round again but it has not expanded (yet, or it failed): that round is
  // the current one even though it has no items, so it is what shows by default.
  const current = live ?? (lastArchived > 0 && own > lastArchived ? own : undefined);
  const rounds = [...archived, ...(current !== undefined ? [current] : [])].sort((a, b) => a - b);
  const list = (round: number): unknown[] | undefined => {
    const found = round === live ? liveItems : items[archivedItemsKey(group.id, round)];
    return Array.isArray(found) ? found : undefined;
  };
  return {
    rounds,
    live,
    list,
    // Branches of the live round are kept under round 0 while counting.
    count: (round) =>
      Math.max(list(round)?.length ?? 0, widest.get(round === live ? 0 : round) ?? 0),
  };
}

function itemTitle(list: unknown[] | undefined, index: number): string {
  const item = list?.[index];
  if (item && typeof item === "object") {
    const fields = item as Record<string, unknown>;
    for (const key of ["title", "name"]) {
      const value = fields[key];
      if (typeof value === "string" && value.trim()) return value.trim();
    }
  }
  return `#${index + 1}`;
}

/** How one item stands, from the nodes of its lane. */
function itemStatus(run: WorkflowRun, runs: NodeRun[]): RunNodeStatus {
  if (runs.length === 0) return "pending";
  const states = runs.map(statusOf);
  for (const first of ["failed", "running", "waiting"] as const) if (states.includes(first)) return first;
  if (states.includes("stopped")) return "stopped";
  const settled = states.filter((s) => s === "done" || s === "skipped");
  if (settled.length === states.length) return states.includes("done") ? "done" : "skipped";
  // Some nodes done and the rest not started: between two nodes of the lane.
  if (settled.length === 0) return "pending";
  return run.status === "running" ? "running" : "stopped";
}

const SUMMARY_ORDER: RunNodeStatus[] = ["done", "running", "waiting", "failed", "skipped", "stopped", "pending"];

function childrenOf(run: WorkflowRun, groupId: NodeId): WorkflowNode[] {
  return run.definition.nodes.filter((n) => n.parent === groupId && n.type !== "forEach");
}

export function groupViews(
  run: WorkflowRun,
  pick: Record<NodeId, { round?: number; item?: number }> = {},
): GroupView[] {
  const byKey = nodeRunsByKey(run);
  return run.definition.nodes
    .filter((n): n is ForEach => n.type === "forEach")
    .map((group) => {
      const info = roundsOf(run, group);
      const wanted = pick[group.id]?.round;
      const round =
        wanted !== undefined && info.rounds.includes(wanted)
          ? wanted
          : (info.rounds[info.rounds.length - 1] ?? byKey.get(nodeRunKey(group.id, ""))?.round ?? 1);
      const list = info.rounds.includes(round) ? info.list(round) : undefined;
      const count = info.rounds.includes(round) ? info.count(round) : 0;
      const kids = childrenOf(run, group.id);
      const items = Array.from({ length: count }, (_, index) => {
        const branch = branchOf(group.id, index, round, info.live);
        const runs = kids.flatMap((k) => byKey.get(nodeRunKey(k.id, branch)) ?? []);
        return { index, title: itemTitle(list, index), status: itemStatus(run, runs) };
      });
      const chosen = pick[group.id]?.item;
      const item =
        chosen !== undefined && chosen >= 0 && chosen < count
          ? chosen
          : Math.max(0, items.findIndex((i) => i.status === "running" || i.status === "waiting"));
      let summary: string;
      if (list !== undefined && list.length === 0) summary = "No items";
      else if (count === 0) {
        // Not expanded in this round: say where the group stands, or why it failed.
        const own = byKey.get(nodeRunKey(group.id, ""));
        summary = own?.status === "failed" && own.error ? own.error : stateOf(byKey, group.id, "", 0).label;
      } else {
        const counts = SUMMARY_ORDER.map((s) => [s, items.filter((i) => i.status === s).length] as const);
        summary = counts.filter(([, n]) => n > 0).map(([s, n]) => `${n} ${s}`).join(" · ");
      }
      return { groupId: group.id, rounds: info.rounds, round, items, item, summary };
    });
}

/** The width of one lane of a group: its children's snapshot layout, left padding left out. */
function laneWidthOf(kids: readonly FlowNode[]): number {
  return Math.max(NODE_W, ...kids.map((k) => k.position.x + NODE_W - GROUP_PAD));
}

/** Lanes when every For each is small (up to 4 items, up to 3 narrow nodes in it), else one lane at a time. */
export function defaultRunView(run: WorkflowRun): RunView {
  const flow = toFlow(run.definition).nodes;
  for (const group of run.definition.nodes) {
    if (group.type !== "forEach") continue;
    // Children laid out side by side make a lane too wide to repeat.
    if (laneWidthOf(flow.filter((n) => n.parentId === group.id)) > NODE_W * 1.5) return "single";
    const info = roundsOf(run, group);
    const items = Math.max(0, ...info.rounds.map((r) => info.count(r)));
    if (items > LANES_MAX_ITEMS || childrenOf(run, group.id).length > LANES_MAX_CHILDREN) return "single";
  }
  return "lanes";
}

// --- edges ------------------------------------------------------------------------------------

function edgeState(def: WorkflowEdge, branch: BranchKey, byKey: Map<string, NodeRun>): RunEdgeState {
  const source = byKey.get(nodeRunKey(def.from, branch));
  const target = byKey.get(nodeRunKey(def.to, branch));
  if (!source) return "idle";
  if (def.loop) {
    if (isAsking(source)) return "active";
    const looped =
      source.decision?.value === "loop" || (source.history ?? []).some((h) => h.decision?.value === "loop");
    return looped ? "taken" : "idle";
  }
  if (source.status !== "done") return "idle";
  if (def.handle && source.decision?.value !== def.handle) return "idle";
  if (target && (target.status === "running" || target.status === "waitingApproval")) return "active";
  return "taken";
}

// --- the graph --------------------------------------------------------------------------------

type Box = { x: number; y: number; w: number; h: number };

/**
 * Makes room for a box that grows by `dx` and `dy` (a group that got lanes) or moves by them:
 * what sits to its right moves right, what sits below moves down, and those moves cascade, so
 * nothing ends up covered. Boxes only ever move right or down, so it ends.
 */
function shove(boxes: Map<NodeId, Box>, id: NodeId, dx: number, dy: number, moves: boolean): void {
  const g = boxes.get(id) as Box;
  const old = { ...g };
  if (dx > 0) {
    for (const [other, b] of boxes) {
      if (other !== id && b.x >= old.x + old.w && b.y < old.y + old.h && b.y + b.h > old.y) shove(boxes, other, dx, 0, true);
    }
  }
  if (dy > 0) {
    for (const [other, b] of boxes) {
      if (other !== id && b.y >= old.y + old.h && b.x < old.x + old.w + dx && b.x + b.w > old.x) shove(boxes, other, 0, dy, true);
    }
  }
  if (moves) {
    g.x += dx;
    g.y += dy;
  } else {
    g.w += dx;
    g.h += dy;
  }
}

export function runGraph(
  run: WorkflowRun,
  view: RunView,
  groups: GroupView[],
  /** The clock for "Running 3m"; a parameter so tests need no fake timers. */
  now: number = Date.now(),
): { nodes: FlowNode[]; edges: (FlowEdge & { data: { loop?: LoopSpec; run: RunEdgeState } })[] } {
  const base = toFlow(run.definition);
  const byKey = nodeRunsByKey(run);
  const groupView = new Map(groups.map((g) => [g.groupId, g]));
  const loopFrom = new Set(run.definition.edges.filter((e) => e.loop).map((e) => e.from));
  const loopTo = new Set(run.definition.edges.filter((e) => e.loop).map((e) => e.to));

  // The branches each group draws a lane for, in order.
  const lanes = new Map<NodeId, { branch: BranchKey; x: number }[]>();
  const growth = new Map<NodeId, { dx: number; dy: number }>();
  const nodes: FlowNode[] = [];
  const boxes = new Map<NodeId, Box>();

  for (const flow of base.nodes) {
    if (flow.parentId) continue;
    const def = flow.data.node;
    const box = { x: flow.position.x, y: flow.position.y, w: flow.width ?? NODE_W, h: flow.height ?? NODE_H };
    boxes.set(flow.id, box);
    if (flow.type === "group") {
      const gv = groupView.get(flow.id);
      const info = roundsOf(run, def as ForEach);
      const kids = base.nodes.filter((n) => n.parentId === flow.id);
      const picked = gv ? (view === "lanes" ? gv.items : gv.items.filter((i) => i.index === gv.item)) : [];
      const picks = kids.length === 0 ? [] : picked;
      const laneW = laneWidthOf(kids);
      lanes.set(
        flow.id,
        picks.map((item, k) => ({
          branch: branchOf(flow.id, item.index, gv?.round ?? 1, info.live),
          x: GROUP_PAD + k * (laneW + LANE_GAP),
        })),
      );
      if (picks.length > 0) growth.set(flow.id, { dx: (picks.length - 1) * (laneW + LANE_GAP), dy: LANE_HEADER });
      nodes.push({
        ...flow,
        data: {
          ...flow.data,
          run: { ...stateOf(byKey, flow.id, "", now), label: gv?.summary ?? stateOf(byKey, flow.id, "", now).label },
          loopIn: loopTo.has(flow.id),
          loopOut: loopFrom.has(flow.id),
        },
      });
    } else {
      nodes.push({
        ...flow,
        data: {
          ...flow.data,
          run: stateOf(byKey, flow.id, "", now),
          loopIn: loopTo.has(flow.id),
          loopOut: loopFrom.has(flow.id),
        },
      });
    }
  }

  // Grow the groups that got lanes, moving whatever they would now cover.
  const order = [...growth.keys()].sort(
    (a, b) => (boxes.get(a)?.y ?? 0) - (boxes.get(b)?.y ?? 0) || (boxes.get(a)?.x ?? 0) - (boxes.get(b)?.x ?? 0),
  );
  for (const id of order) {
    const { dx, dy } = growth.get(id) as { dx: number; dy: number };
    shove(boxes, id, dx, dy, false);
  }
  for (const n of nodes) {
    const box = boxes.get(n.id);
    if (!box) continue;
    n.position = { x: box.x, y: box.y };
    if (n.type === "group") {
      n.width = box.w;
      n.height = box.h;
    }
  }

  // The nodes inside the lanes.
  for (const flow of base.nodes) {
    if (!flow.parentId) continue;
    for (const lane of lanes.get(flow.parentId) ?? []) {
      nodes.push({
        ...flow,
        id: `${flow.id}@${lane.branch}`,
        position: { x: lane.x + (flow.position.x - GROUP_PAD), y: flow.position.y + LANE_HEADER },
        data: {
          ...flow.data,
          run: stateOf(byKey, flow.id, lane.branch, now),
          loopIn: loopTo.has(flow.id),
          loopOut: loopFrom.has(flow.id),
        },
      });
    }
  }

  const parentOf = new Map(base.nodes.map((n) => [n.id, n.parentId]));
  const edges: ReturnType<typeof runGraph>["edges"] = [];
  base.edges.forEach((flow, i) => {
    const def = run.definition.edges[i];
    const from = parentOf.get(def.from);
    const to = parentOf.get(def.to);
    if (from === undefined && to === undefined) {
      edges.push({ ...flow, data: { loop: def.loop, run: edgeState(def, "", byKey) } });
    } else if (from !== undefined && from === to) {
      for (const lane of lanes.get(from) ?? []) {
        edges.push({
          ...flow,
          id: `${flow.id}@${lane.branch}`,
          source: `${def.from}@${lane.branch}`,
          target: `${def.to}@${lane.branch}`,
          data: { loop: def.loop, run: edgeState(def, lane.branch, byKey) },
        });
      }
    }
    // An edge between a lane and the outside has nowhere to attach and is left out.
  });
  return { nodes, edges };
}
