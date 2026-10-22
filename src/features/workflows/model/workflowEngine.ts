// Pure run state machine for workflows: no I/O, no timers. The runner feeds it events and executes
// the actions it returns.
//
// Contract with the runner:
// - A node that is `running` with `endedAt` set is not running anything: it has finished and the
//   Mono is being asked whether to loop (see `isAsking`). The runner must not apply a session or
//   command timeout to it and must not run it again.
// - `startRun`, `applyEvent`, `resumeRun` return a run that is already "settled": every step the
//   engine can take by itself (triggers, If conditions, For each expansion, skipping untaken
//   sides, failing a branch, condition loops, the final status) has been applied.
// - `nextActions` only reads. It lists what the runner must do for nodes that are ready. An action
//   is repeated until the matching `nodeStarted` event is applied, and never after.
import {
  isTrigger,
  type BranchKey,
  type LoopSpec,
  type NodeId,
  type NodeRun,
  type NodeStatus,
  type RunStatus,
  type Workflow,
  type WorkflowEdge,
  type WorkflowNode,
  type WorkflowRun,
} from "./workflow";
import { evalCondition } from "./workflowCondition";
import { outputInstruction } from "./workflowOutput";
import {
  lookup,
  renderTemplate,
  shellTemplateProblems,
  type TemplateScope,
} from "./workflowTemplate";

export type { BranchKey, NodeRun, NodeStatus, RunStatus, WorkflowRun };

export type EngineAction =
  | {
      kind: "startSession";
      nodeId: NodeId;
      branch: BranchKey;
      round: number;
      prompt: string;
      reuseSessionId?: string;
    }
  | { kind: "runCommand"; nodeId: NodeId; branch: BranchKey; command: string }
  | {
      kind: "askMono";
      nodeId: NodeId;
      branch: BranchKey;
      purpose: "askMono" | "if" | "loop";
      question: string;
      answers: string[];
      context: Record<string, unknown>;
    }
  | { kind: "postReport" };

export type EngineEvent =
  | { kind: "nodeStarted"; nodeId: NodeId; branch: BranchKey; sessionId?: string; at: number }
  | {
      kind: "nodeFinished";
      nodeId: NodeId;
      branch: BranchKey;
      output: Record<string, unknown>;
      at: number;
    }
  | { kind: "nodeFailed"; nodeId: NodeId; branch: BranchKey; error: string; at: number }
  | {
      kind: "decided";
      nodeId: NodeId;
      branch: BranchKey;
      value: string;
      reason?: string;
      at: number;
    }
  | { kind: "waitingApproval"; nodeId: NodeId; branch: BranchKey; waiting: boolean }
  | { kind: "reportPosted"; at: number }
  | { kind: "stopped"; at: number };

const DEFAULT_MAX_PARALLEL = 3;
const MAX_ROUNDS_REASON = "max rounds reached";
const BRANCH_FAILED_REASON = "branch failed";

// --- graph ----------------------------------------------------------------------------------

type LoopEdge = { to: NodeId; loop: LoopSpec };

type Graph = {
  byId: Map<NodeId, WorkflowNode>;
  /** Non-loop edges whose endpoints exist and share a parent, by target. */
  incoming: Map<NodeId, WorkflowEdge[]>;
  outgoing: Map<NodeId, NodeId[]>;
  /** Children of each For each node, in definition order. */
  children: Map<NodeId, WorkflowNode[]>;
  /** The loop edge leaving a node, when it can run one. */
  loops: Map<NodeId, LoopEdge>;
};

const graphs = new WeakMap<Workflow, Graph>();

function graphOf(def: Workflow): Graph {
  const cached = graphs.get(def);
  if (cached) return cached;
  const byId = new Map<NodeId, WorkflowNode>();
  for (const n of def.nodes) if (!byId.has(n.id)) byId.set(n.id, n);
  const incoming = new Map<NodeId, WorkflowEdge[]>();
  const outgoing = new Map<NodeId, NodeId[]>();
  const loops = new Map<NodeId, LoopEdge>();
  for (const e of def.edges) {
    const from = byId.get(e.from);
    const to = byId.get(e.to);
    // Edges that cross a For each boundary are not supported: the group is the boundary.
    if (!from || !to || from.parent !== to.parent) continue;
    if (e.loop) {
      // Only nodes that finish with a value the decider can look at can loop.
      const loopable = from.type === "session" || from.type === "command" || from.type === "askMono";
      if (loopable && !loops.has(e.from)) loops.set(e.from, { to: e.to, loop: e.loop });
      continue;
    }
    incoming.set(e.to, [...(incoming.get(e.to) ?? []), e]);
    outgoing.set(e.from, [...(outgoing.get(e.from) ?? []), e.to]);
  }
  const children = new Map<NodeId, WorkflowNode[]>();
  for (const n of def.nodes) {
    if (n.parent === undefined) continue;
    children.set(n.parent, [...(children.get(n.parent) ?? []), n]);
  }
  const g = { byId, incoming, outgoing, children, loops };
  graphs.set(def, g);
  return g;
}

function reach(start: NodeId, next: (id: NodeId) => Iterable<NodeId>): Set<NodeId> {
  const seen = new Set<NodeId>();
  const queue = [start];
  while (queue.length) {
    for (const n of next(queue.pop() as NodeId)) {
      if (!seen.has(n)) {
        seen.add(n);
        queue.push(n);
      }
    }
  }
  return seen;
}

/** `to`, `from` and every node between them over non-loop edges. */
function loopBody(g: Graph, from: NodeId, to: NodeId): NodeId[] {
  const below = reach(to, (id) => g.outgoing.get(id) ?? []);
  const above = reach(from, (id) => (g.incoming.get(id) ?? []).map((e) => e.from));
  const body = new Set<NodeId>([to, from]);
  for (const id of below) if (above.has(id)) body.add(id);
  return [...body];
}

/** Longest distance from a node with no predecessors; breaks ties between equal end times. */
function depthOf(g: Graph, id: NodeId, memo = new Map<NodeId, number>()): number {
  const known = memo.get(id);
  if (known !== undefined) return known;
  memo.set(id, 0); // guards against a cycle in a hand-edited definition
  let depth = 0;
  for (const e of g.incoming.get(id) ?? []) depth = Math.max(depth, depthOf(g, e.from, memo) + 1);
  memo.set(id, depth);
  return depth;
}

// --- draft ----------------------------------------------------------------------------------

type Draft = {
  run: WorkflowRun;
  g: Graph;
  index: Map<string, NodeRun>;
  /** Timestamp stamped on every change the engine makes by itself. */
  at: number;
};

const slot = (nodeId: NodeId, branch: BranchKey) => `${nodeId}\u0000${branch}`;

function draftOf(run: WorkflowRun, at: number): Draft {
  const nodes = run.nodes.map((n) => ({ ...n }));
  return {
    run: { ...run, nodes, items: { ...run.items } },
    g: graphOf(run.definition),
    index: new Map(nodes.map((n) => [slot(n.nodeId, n.branch), n])),
    at,
  };
}

function lastActivity(run: WorkflowRun): number {
  let at = run.startedAt;
  for (const n of run.nodes) at = Math.max(at, n.startedAt ?? 0, n.endedAt ?? 0);
  return Math.max(at, run.endedAt ?? 0, run.reportPostedAt ?? 0);
}

const newNodeRun = (nodeId: NodeId, branch: BranchKey): NodeRun => ({
  nodeId,
  branch,
  status: "pending",
  round: 1,
});

// --- small predicates ------------------------------------------------------------------------

/** A finished node whose loop edge has not been decided yet (exit or loop). */
function openLoop(g: Graph, r: NodeRun): boolean {
  return r.status === "done" && r.decision === undefined && g.loops.has(r.nodeId);
}

/** A finished node whose loop edge is being put to the Mono (the node itself is not running). */
export function isAsking(r: NodeRun): boolean {
  return r.status === "running" && r.endedAt !== undefined;
}

function unsettled(g: Graph, r: NodeRun): boolean {
  return (
    r.status === "pending" ||
    r.status === "running" ||
    r.status === "waitingApproval" ||
    openLoop(g, r)
  );
}

const isReport = (g: Graph, r: NodeRun) => g.byId.get(r.nodeId)?.type === "report";

function branchKey(forEachId: NodeId, index: number): BranchKey {
  return `${forEachId}#${index}`;
}

function splitBranch(branch: BranchKey): { forEachId: NodeId; index: number } | undefined {
  const at = branch.lastIndexOf("#");
  if (at < 0) return undefined;
  return { forEachId: branch.slice(0, at), index: Number(branch.slice(at + 1)) };
}

// --- scope -----------------------------------------------------------------------------------

/**
 * Template scope for a node running in `branch`: `inputs`, `run`, the loop variable inside a
 * For each, and the latest output of every node in the branch or at the top level. With `nodeId`,
 * `run.round` is that node's current round (1 outside loops).
 */
export function runScope(run: WorkflowRun, branch: BranchKey, nodeId?: NodeId): TemplateScope {
  const scope: TemplateScope = {};
  let round = 1;
  for (const n of run.nodes) {
    if (n.branch !== "" && n.branch !== branch) continue;
    if (n.output !== undefined) scope[n.nodeId] = n.output;
    if (nodeId !== undefined && n.nodeId === nodeId && n.branch === branch) round = n.round;
  }
  const split = splitBranch(branch);
  if (split) {
    const group = graphOf(run.definition).byId.get(split.forEachId);
    const name = group?.type === "forEach" ? (group.as ?? "item") : "item";
    scope[name] = run.items[split.forEachId]?.[split.index];
  }
  scope.inputs = run.definition.inputValues;
  scope.run = { id: run.id, startedAt: run.startedAt, round };
  return scope;
}

/** Outputs the Mono needs to decide: the named nodes, the loop variable and the round. */
function contextFor(run: WorkflowRun, r: NodeRun, ids: NodeId[]): Record<string, unknown> {
  const scope = runScope(run, r.branch, r.nodeId);
  const context: Record<string, unknown> = {};
  for (const id of ids) if (scope[id] !== undefined) context[id] = scope[id];
  const split = splitBranch(r.branch);
  if (split) {
    const group = graphOf(run.definition).byId.get(split.forEachId);
    const name = group?.type === "forEach" ? (group.as ?? "item") : "item";
    context[name] = scope[name];
  }
  context.round = r.round;
  return context;
}

// --- admission of For each branches -----------------------------------------------------------

/**
 * Branches allowed to make progress. Top level always; inside a For each every branch that has
 * started and not finished, plus as many untouched branches (in index order) as `maxParallel`
 * leaves room for.
 */
function admittedBranches(d: Draft): Set<BranchKey> {
  const admitted = new Set<BranchKey>([""]);
  const byBranch = new Map<BranchKey, NodeRun[]>();
  for (const n of d.run.nodes) byBranch.set(n.branch, [...(byBranch.get(n.branch) ?? []), n]);
  for (const node of d.run.definition.nodes) {
    const items = node.type === "forEach" ? d.run.items[node.id] : undefined;
    if (node.type !== "forEach" || !items) continue;
    const max = node.maxParallel ?? DEFAULT_MAX_PARALLEL;
    let active = 0;
    const waiting: BranchKey[] = [];
    for (let i = 0; i < items.length; i++) {
      const key = branchKey(node.id, i);
      const runs = byBranch.get(key) ?? [];
      if (!runs.some((r) => unsettled(d.g, r))) continue;
      if (runs.some((r) => r.status !== "pending" || r.round > 1 || r.output !== undefined)) {
        admitted.add(key);
        active++;
      } else {
        waiting.push(key);
      }
    }
    for (const key of waiting) {
      if (active >= max) break;
      admitted.add(key);
      active++;
    }
  }
  return admitted;
}

// --- readiness --------------------------------------------------------------------------------

type Gate = "ready" | "blocked" | "dead";

/**
 * Whether a pending node can start. Ready when every predecessor in the same branch is done
 * (an If predecessor counts only along the matching handle). Dead when no predecessor leads
 * to it, so it is skipped. Blocked while a predecessor is still to finish.
 */
function gateOf(d: Draft, r: NodeRun): Gate {
  const edges = d.g.incoming.get(r.nodeId) ?? [];
  if (edges.length === 0) return "ready";
  let live = 0;
  for (const e of edges) {
    const src = d.index.get(slot(e.from, r.branch));
    if (!src) continue;
    if (src.status === "done") {
      if (openLoop(d.g, src)) return "blocked";
      if (d.g.byId.get(e.from)?.type === "if") {
        if (src.decision?.value === e.handle) live++;
      } else {
        live++;
      }
    } else if (src.status !== "skipped" && src.status !== "failed" && src.status !== "stopped") {
      return "blocked";
    }
  }
  return live > 0 ? "ready" : "dead";
}

// --- state changes the engine makes by itself --------------------------------------------------

/** A failed node ends its branch for good: it must not start another loop round. */
function branchFailed(d: Draft, branch: BranchKey): boolean {
  return d.run.nodes.some((n) => n.branch === branch && n.status === "failed");
}

/** Fails the node and skips whatever is still pending in its branch (other branches go on). */
function failNode(d: Draft, r: NodeRun, error: string): void {
  r.status = "failed";
  r.error = error;
  r.startedAt ??= d.at;
  r.endedAt = d.at;
  for (const o of d.run.nodes) {
    if (o.branch !== r.branch || o.status !== "pending" || isReport(d.g, o)) continue;
    o.status = "skipped";
    o.endedAt = d.at;
  }
}

function finishNode(d: Draft, r: NodeRun, output?: Record<string, unknown>): void {
  r.status = "done";
  r.startedAt ??= d.at;
  r.endedAt = d.at;
  if (output !== undefined) r.output = output;
}

/**
 * Sends the branch back to `to`: that node, `from` and everything between them become pending
 * for the next round. Their outputs stay until they run again, so the next round can read the
 * previous one. Session ids stay too, so a looped session can be continued.
 */
function resetLoop(d: Draft, from: NodeRun, to: NodeId): void {
  const round = from.round + 1;
  const dropped = new Set<NodeId>();
  for (const id of loopBody(d.g, from.nodeId, to)) {
    const r = d.index.get(slot(id, from.branch));
    if (!r) continue;
    r.status = "pending";
    r.round = round;
    delete r.error;
    delete r.decision;
    delete r.startedAt;
    delete r.endedAt;
    delete r.reasked;
    if (d.g.byId.get(id)?.type === "forEach") dropped.add(id);
  }
  // Anything downstream of `to` that an earlier round skipped (the untaken side of an If) gets
  // another chance; the gate logic skips it again when it is still dead.
  for (const id of reach(to, (x) => d.g.outgoing.get(x) ?? [])) {
    const r = d.index.get(slot(id, from.branch));
    if (!r || r.status !== "skipped" || d.g.byId.get(id)?.type === "report") continue;
    r.status = "pending";
    r.round = round;
    delete r.endedAt;
  }
  if (dropped.size === 0) return;
  // A For each inside the loop body expands again next round, with a fresh item list.
  const group = new Set<NodeId>();
  for (const id of dropped) {
    delete d.run.items[id];
    for (const c of d.g.children.get(id) ?? []) group.add(c.id);
  }
  d.run.nodes = d.run.nodes.filter((n) => {
    const split = splitBranch(n.branch);
    const gone = split !== undefined && dropped.has(split.forEachId) && group.has(n.nodeId);
    if (gone) d.index.delete(slot(n.nodeId, n.branch));
    return !gone;
  });
}

/** Applies a loop decision to a finished node: another round, or exit. */
function decideLoop(d: Draft, r: NodeRun, value: string, reason?: string): void {
  const spec = d.g.loops.get(r.nodeId);
  if (!spec) return;
  if (value === "loop" && branchFailed(d, r.branch)) {
    // The question was already in flight when a node of the branch failed.
    r.status = "done";
    r.decision = { value: "exit", reason: BRANCH_FAILED_REASON };
  } else if (value === "loop" && r.round < spec.loop.maxRounds) {
    r.status = "done";
    resetLoop(d, r, spec.to);
  } else if (value === "exit" || value === "loop") {
    // A "loop" answer at the last round is treated as exit: the app always exits there.
    r.status = "done";
    r.decision =
      value === "loop"
        ? { value: "exit", reason: MAX_ROUNDS_REASON }
        : { value: "exit", ...(reason !== undefined ? { reason } : {}) };
  } else {
    failNode(d, r, `Invalid loop decision "${value}" (expected loop or exit)`);
  }
}

function lastOutput(d: Draft, group: NodeId, branch: BranchKey): unknown {
  let best: NodeRun | undefined;
  let bestKey: [number, number, number] = [-1, -1, -1];
  const kids = d.g.children.get(group) ?? [];
  kids.forEach((kid, order) => {
    const r = d.index.get(slot(kid.id, branch));
    if (!r || r.status !== "done" || r.output === undefined) return;
    const key: [number, number, number] = [r.endedAt ?? 0, depthOf(d.g, kid.id), order];
    if (
      key[0] > bestKey[0] ||
      (key[0] === bestKey[0] && (key[1] > bestKey[1] || (key[1] === bestKey[1] && key[2] > bestKey[2])))
    ) {
      best = r;
      bestKey = key;
    }
  });
  return best?.output ?? null;
}

const LIST_EXPRESSION = /^\s*\{\{\s*([^{}\s]+)\s*\}\}\s*$/;

function expandForEach(d: Draft, r: NodeRun, node: Extract<WorkflowNode, { type: "forEach" }>) {
  const match = LIST_EXPRESSION.exec(node.in);
  const value = match ? lookup(runScope(d.run, r.branch, r.nodeId), match[1]) : undefined;
  if (!Array.isArray(value)) {
    failNode(d, r, `Expected a list for ${node.in}`);
    return;
  }
  d.run.items[node.id] = value;
  r.startedAt ??= d.at;
  if (value.length === 0) {
    finishNode(d, r, { results: [] });
    return;
  }
  r.status = "running";
  for (let i = 0; i < value.length; i++) {
    for (const child of d.g.children.get(node.id) ?? []) {
      const run = newNodeRun(child.id, branchKey(node.id, i));
      d.run.nodes.push(run);
      d.index.set(slot(run.nodeId, run.branch), run);
    }
  }
}

/** One pass of everything the engine decides on its own. Returns whether anything changed. */
function pass(d: Draft): boolean {
  let changed = false;
  const admitted = admittedBranches(d);
  for (const r of [...d.run.nodes]) {
    const node = d.g.byId.get(r.nodeId);
    if (!node || !d.index.has(slot(r.nodeId, r.branch))) continue;

    if (openLoop(d.g, r)) {
      const spec = d.g.loops.get(r.nodeId) as LoopEdge;
      if (branchFailed(d, r.branch)) {
        r.decision = { value: "exit", reason: BRANCH_FAILED_REASON };
        changed = true;
      } else if (r.round >= spec.loop.maxRounds) {
        r.decision = { value: "exit", reason: MAX_ROUNDS_REASON };
        changed = true;
      } else if (spec.loop.decide !== "mono") {
        const result = evalCondition(spec.loop.decide, runScope(d.run, r.branch, r.nodeId));
        if (!result.ok) failNode(d, r, result.error);
        else if (result.value) resetLoop(d, r, spec.to);
        else r.decision = { value: "exit" };
        changed = true;
      }
      continue;
    }

    if (node.type === "forEach" && r.status === "running") {
      const items = d.run.items[node.id] ?? [];
      let finished = true;
      for (let i = 0; i < items.length && finished; i++) {
        const key = branchKey(node.id, i);
        finished = !d.run.nodes.some((n) => n.branch === key && unsettled(d.g, n));
      }
      if (finished) {
        const results = items.map((_, i) => lastOutput(d, node.id, branchKey(node.id, i)));
        finishNode(d, r, { results });
        changed = true;
      }
      continue;
    }

    if (r.status !== "pending" || node.type === "report") continue;
    if (r.branch !== "" && !admitted.has(r.branch)) continue;
    const gate = gateOf(d, r);
    if (gate === "blocked") continue;
    if (gate === "dead") {
      r.status = "skipped";
      r.endedAt = d.at;
      changed = true;
      continue;
    }

    switch (node.type) {
      case "schedule":
      case "manual":
        finishNode(d, r, {});
        changed = true;
        break;
      case "forEach":
        if (node.parent !== undefined) failNode(d, r, "Nested For each is not supported");
        else expandForEach(d, r, node);
        changed = true;
        break;
      case "if":
        if (node.decide !== "mono") {
          const result = evalCondition(node.condition ?? "", runScope(d.run, r.branch, r.nodeId));
          if (!result.ok) {
            failNode(d, r, result.error);
          } else {
            r.decision = { value: String(result.value) };
            finishNode(d, r);
          }
          changed = true;
        }
        break;
      case "command": {
        const problems = shellTemplateProblems(node.command);
        if (problems.length > 0) {
          failNode(d, r, problems.join("; "));
          changed = true;
        }
        break;
      }
      default:
        break; // session and askMono are the runner's to start
    }
  }
  return changed;
}

function finalStatus(d: Draft): RunStatus {
  const failed = d.run.nodes.some((n) => n.status === "failed");
  if (!failed) return "done";
  const succeeded = d.run.nodes.some((n) => {
    const type = d.g.byId.get(n.nodeId)?.type;
    return n.status === "done" && (type === "session" || type === "command");
  });
  return succeeded ? "doneWithFailures" : "failed";
}

/** Applies every step the engine can take alone, then ends the run when nothing is left. */
function settle(d: Draft): void {
  if (d.run.status !== "running") return;
  for (let guard = 0; guard < 10_000 && pass(d); guard++);
  if (d.run.nodes.some((n) => !isReport(d.g, n) && unsettled(d.g, n))) return;
  // Everything else is finished; the report still has to be posted.
  if (d.run.nodes.some((n) => isReport(d.g, n) && n.status === "pending")) return;
  d.run.status = finalStatus(d);
  d.run.endedAt = d.at;
}

// --- public API --------------------------------------------------------------------------------

export function startRun(
  def: Workflow,
  trigger: WorkflowRun["trigger"],
  now: number,
  id: string,
): WorkflowRun {
  const definition = structuredClone(def);
  const g = graphOf(definition);
  const top = definition.nodes.filter((n) => n.parent === undefined);
  const roots = top.filter(isTrigger);
  // Without a trigger (an invalid definition) start from the nodes nothing leads to.
  const starts =
    roots.length > 0
      ? roots
      : top.filter((n) => n.type !== "report" && (g.incoming.get(n.id) ?? []).length === 0);
  const reachable = new Set<NodeId>(starts.map((n) => n.id));
  for (const s of starts) for (const n of reach(s.id, (x) => g.outgoing.get(x) ?? [])) reachable.add(n);

  const run: WorkflowRun = {
    id,
    workflowId: definition.id,
    definition,
    status: "running",
    startedAt: now,
    trigger,
    items: {},
    nodes: top.map((n) => {
      const r = newNodeRun(n.id, "");
      if (!reachable.has(n.id)) r.status = "skipped";
      return r;
    }),
  };
  const d = draftOf(run, now);
  // A schedule trigger exposes when it fired; a manual one has nothing to expose.
  for (const n of top) {
    const r = d.index.get(slot(n.id, ""));
    if (n.type === "schedule" && r?.status === "pending") {
      finishNode(d, r, { startedAt: now });
    }
  }
  settle(d);
  return d.run;
}

function describeNode(node: WorkflowNode): string {
  return `"${node.title ?? node.id}"`;
}

export function nextActions(run: WorkflowRun): EngineAction[] {
  if (run.status !== "running") return [];
  const d = draftOf(run, lastActivity(run));
  const admitted = admittedBranches(d);
  const actions: EngineAction[] = [];

  for (const r of d.run.nodes) {
    const node = d.g.byId.get(r.nodeId);
    if (!node) continue;

    if (openLoop(d.g, r)) {
      const spec = d.g.loops.get(r.nodeId) as LoopEdge;
      if (spec.loop.decide !== "mono" || r.round >= spec.loop.maxRounds) continue;
      const to = d.g.byId.get(spec.to);
      actions.push({
        kind: "askMono",
        nodeId: r.nodeId,
        branch: r.branch,
        purpose: "loop",
        question:
          `${describeNode(node)} finished round ${r.round} of ${spec.loop.maxRounds}. ` +
          `Should the work go back to ${to ? describeNode(to) : spec.to} for another round, or exit?`,
        answers: ["loop", "exit"],
        context: contextFor(d.run, r, loopBody(d.g, r.nodeId, spec.to)),
      });
      continue;
    }

    if (r.status !== "pending" || (r.branch !== "" && !admitted.has(r.branch))) continue;
    if (gateOf(d, r) !== "ready") continue;
    const scope = runScope(d.run, r.branch, r.nodeId);
    const before = (d.g.incoming.get(r.nodeId) ?? []).map((e) => e.from);

    switch (node.type) {
      case "session": {
        const instruction = outputInstruction(node.output);
        const prompt = renderTemplate(node.prompt, scope);
        actions.push({
          kind: "startSession",
          nodeId: r.nodeId,
          branch: r.branch,
          round: r.round,
          prompt: instruction ? `${prompt}\n\n${instruction}` : prompt,
          ...(node.reuseSession !== false && r.round > 1 && r.sessionId
            ? { reuseSessionId: r.sessionId }
            : {}),
        });
        break;
      }
      case "command":
        // Commands with unsafe quoting were failed while settling, so none reach this point.
        actions.push({
          kind: "runCommand",
          nodeId: r.nodeId,
          branch: r.branch,
          command: renderTemplate(node.command, scope, { shell: true }),
        });
        break;
      case "askMono":
        actions.push({
          kind: "askMono",
          nodeId: r.nodeId,
          branch: r.branch,
          purpose: "askMono",
          question: renderTemplate(node.question, scope),
          answers: [...node.answers],
          context: contextFor(d.run, r, before),
        });
        break;
      case "if":
        if (node.decide === "mono") {
          actions.push({
            kind: "askMono",
            nodeId: r.nodeId,
            branch: r.branch,
            purpose: "if",
            question: node.question
              ? renderTemplate(node.question, scope)
              : `Should the "true" side of ${describeNode(node)} be taken? Answer true or false.`,
            answers: ["true", "false"],
            context: contextFor(d.run, r, before),
          });
        }
        break;
      default:
        break;
    }
  }

  const reportPending = d.run.nodes.some((n) => isReport(d.g, n) && n.status === "pending");
  if (
    reportPending &&
    run.reportPostedAt === undefined &&
    !d.run.nodes.some((n) => !isReport(d.g, n) && unsettled(d.g, n))
  ) {
    actions.push({ kind: "postReport" });
  }
  return actions;
}

function stopRun(run: WorkflowRun, at: number): WorkflowRun {
  if (run.status !== "running" && run.status !== "interrupted") return run;
  return {
    ...run,
    status: "stopped",
    endedAt: at,
    nodes: run.nodes.map((n) => {
      if (n.status === "running" || n.status === "waitingApproval") {
        return { ...n, status: "stopped", endedAt: at };
      }
      if (n.status === "pending") return { ...n, status: "skipped", endedAt: at };
      return n;
    }),
  };
}

const UNSTARTED: NodeStatus[] = ["pending", "running", "waitingApproval"];

export function applyEvent(run: WorkflowRun, event: EngineEvent): WorkflowRun {
  if (event.kind === "stopped") return stopRun(run, event.at);
  if (run.status !== "running") return run;
  const d = draftOf(run, "at" in event ? event.at : lastActivity(run));

  if (event.kind === "reportPosted") {
    const report = d.run.nodes.find((n) => isReport(d.g, n) && n.status === "pending");
    if (report) finishNode(d, report);
    d.run.reportPostedAt = event.at;
    settle(d);
    return d.run;
  }

  const r = d.index.get(slot(event.nodeId, event.branch));
  const node = d.g.byId.get(event.nodeId);
  if (!r || !node) return run;

  switch (event.kind) {
    case "nodeStarted":
      if (r.status === "pending") {
        r.status = "running";
        r.startedAt = event.at;
        delete r.endedAt;
        if (event.sessionId !== undefined) r.sessionId = event.sessionId;
      } else if (openLoop(d.g, r)) {
        // The loop question went out: the node stays finished but is no longer to be re-asked.
        r.status = "running";
      } else {
        return run;
      }
      break;
    case "nodeFinished":
      if (!UNSTARTED.includes(r.status) || isAsking(r)) return run;
      finishNode(d, r, event.output);
      break;
    case "nodeFailed":
      if (r.status === "done" || r.status === "failed" || r.status === "skipped") return run;
      failNode(d, r, event.error);
      break;
    case "waitingApproval":
      if (event.waiting && r.status === "running" && !isAsking(r)) r.status = "waitingApproval";
      else if (!event.waiting && r.status === "waitingApproval") r.status = "running";
      else return run;
      break;
    case "decided": {
      if (d.g.loops.has(r.nodeId) && (isAsking(r) || openLoop(d.g, r))) {
        decideLoop(d, r, event.value, event.reason);
      } else if (!UNSTARTED.includes(r.status)) {
        return run;
      } else if (node.type === "if") {
        if (event.value !== "true" && event.value !== "false") {
          failNode(d, r, `Invalid decision "${event.value}" (expected true or false)`);
        } else {
          r.decision = {
            value: event.value,
            ...(event.reason !== undefined ? { reason: event.reason } : {}),
          };
          finishNode(d, r);
        }
      } else if (node.type === "askMono") {
        if (!node.answers.includes(event.value)) {
          failNode(d, r, `Mono answered "${event.value}", which is not an allowed answer`);
        } else {
          finishNode(d, r, { answer: event.value, reason: event.reason ?? "" });
        }
      } else {
        return run;
      }
      break;
    }
  }
  settle(d);
  return d.run;
}

/** Running nodes go back to pending; finished nodes stay finished. */
export function markInterrupted(run: WorkflowRun): WorkflowRun {
  if (run.status !== "running") return run;
  const g = graphOf(run.definition);
  return {
    ...run,
    status: "interrupted",
    nodes: run.nodes.map((n) => {
      if (n.status !== "running" && n.status !== "waitingApproval") return n;
      // A For each group is bookkeeping: its branches carry the real state.
      if (g.byId.get(n.nodeId)?.type === "forEach") return n;
      // Waiting on the Mono about a loop: the node itself had finished, only the question is lost.
      if (isAsking(n)) return { ...n, status: "done" };
      const { sessionId: _sessionId, startedAt: _startedAt, ...rest } = n;
      return { ...rest, status: "pending" };
    }),
  };
}

export function resumeRun(run: WorkflowRun): WorkflowRun {
  if (run.status !== "interrupted") return run;
  const { endedAt: _endedAt, ...rest } = run;
  const d = draftOf({ ...rest, status: "running" }, lastActivity(run));
  settle(d);
  return d.run;
}
