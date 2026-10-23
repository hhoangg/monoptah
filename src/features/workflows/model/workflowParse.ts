import { habitSchedule, type HabitSchedule } from "../../monos/model/monoHabits";
import {
  MAX_TIMEOUT_MINUTES,
  NODE_ID,
  NODE_TYPES,
  isTrigger,
  type Checkout,
  type NodeId,
  type NodeType,
  type OutputField,
  type OutputFieldType,
  type Workflow,
  type WorkflowEdge,
  type WorkflowInput,
  type WorkflowNode,
} from "./workflow";
import { shellTemplateProblems, templateVars } from "./workflowTemplate";

export type WorkflowIssue = { nodeId?: NodeId; message: string };

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

type Rec = Record<string, unknown>;

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
/**
 * Whether `name` may be a key of an input or output map. Names end up in `{{a.b}}` paths, and
 * "__proto__" would replace the prototype of a plain object when a parsed map is built.
 */
const isMapKey = (name: string) => NAME.test(name) && name !== "__proto__";

const isRec = (v: unknown): v is Rec => !!v && typeof v === "object" && !Array.isArray(v);
const isString = (v: unknown): v is string => typeof v === "string";
const isNumber = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isBool = (v: unknown): v is boolean => typeof v === "boolean";

function str(v: Rec, key: string): string | undefined {
  return isString(v[key]) ? v[key] : undefined;
}
function num(v: Rec, key: string): number | undefined {
  return isNumber(v[key]) ? v[key] : undefined;
}
function bool(v: Rec, key: string): boolean | undefined {
  return isBool(v[key]) ? v[key] : undefined;
}
/** `{ key: value }` for a defined value, `{}` otherwise, so optional fields stay absent. */
function opt<K extends string, T>(key: K, value: T | undefined): { [P in K]?: T } {
  return value === undefined ? {} : ({ [key]: value } as { [P in K]?: T });
}

const OUTPUT_TYPES: readonly OutputFieldType[] = ["string", "number", "boolean", "list", "object"];
const INPUT_TYPES: readonly WorkflowInput["type"][] = ["project", "string", "number", "boolean"];

function parseSchedule(raw: unknown): HabitSchedule | undefined {
  if (!isRec(raw)) return undefined;
  const kind = str(raw, "scheduleKind");
  if (!kind) return undefined;
  return {
    // Whether the kind is one a habit knows is validateWorkflow's call, so a draft survives.
    scheduleKind: kind as HabitSchedule["scheduleKind"],
    time: str(raw, "time") ?? "09:00",
    minute: num(raw, "minute") ?? 0,
    dayOfWeek: num(raw, "dayOfWeek") ?? 1,
    ...opt("intervalHours", num(raw, "intervalHours")),
    ...opt("intervalFrom", str(raw, "intervalFrom")),
  };
}

function parseCheckout(raw: unknown): Checkout | undefined {
  if (raw === "root" || raw === "newWorktree") return raw;
  if (isRec(raw) && isString(raw.worktreeOf)) return { worktreeOf: raw.worktreeOf };
  return undefined;
}

function parseOutput(raw: unknown): Record<string, OutputField> | undefined {
  if (raw === undefined) return {};
  if (!isRec(raw)) return undefined;
  const output: Record<string, OutputField> = {};
  for (const [name, field] of Object.entries(raw)) {
    if (!isMapKey(name) || !isRec(field) || !OUTPUT_TYPES.includes(field.type as OutputFieldType)) continue;
    output[name] = {
      type: field.type as OutputFieldType,
      ...opt("inReport", bool(field, "inReport")),
      ...opt("description", str(field, "description")),
    };
  }
  return output;
}

export function parseNode(raw: unknown): WorkflowNode | undefined {
  if (!isRec(raw)) return undefined;
  const id = str(raw, "id");
  const type = raw.type;
  if (!id || !isString(type) || !NODE_TYPES.includes(type as NodeType)) return undefined;
  const ui = raw.ui;
  const base = {
    id,
    ...opt("title", str(raw, "title")),
    ...opt("parent", str(raw, "parent")),
    ...(isRec(ui) && isNumber(ui.x) && isNumber(ui.y) ? { ui: { x: ui.x, y: ui.y } } : {}),
  };
  switch (type as NodeType) {
    case "schedule": {
      const schedule = parseSchedule(raw.schedule);
      return schedule ? { ...base, type: "schedule", schedule } : undefined;
    }
    case "manual":
      return { ...base, type: "manual" };
    case "session": {
      const project = str(raw, "project");
      const prompt = str(raw, "prompt");
      const checkout = parseCheckout(raw.checkout);
      const output = parseOutput(raw.output);
      if (project === undefined || prompt === undefined || !checkout || !output) return undefined;
      return {
        ...base,
        type: "session",
        project,
        prompt,
        checkout,
        output,
        ...opt("harness", str(raw, "harness")),
        ...opt("model", str(raw, "model")),
        ...opt("effort", str(raw, "effort")),
        ...opt("reuseSession", bool(raw, "reuseSession")),
        ...opt("nestUnder", str(raw, "nestUnder")),
        ...opt("timeoutMinutes", num(raw, "timeoutMinutes")),
      };
    }
    case "command": {
      const project = str(raw, "project");
      const command = str(raw, "command");
      if (project === undefined || command === undefined) return undefined;
      const cwd =
        raw.cwd === "root"
          ? ("root" as const)
          : isRec(raw.cwd) && isString(raw.cwd.worktreeOf)
            ? { worktreeOf: raw.cwd.worktreeOf }
            : undefined;
      return {
        ...base,
        type: "command",
        project,
        command,
        ...opt("cwd", cwd),
        ...opt("timeoutMinutes", num(raw, "timeoutMinutes")),
      };
    }
    case "askMono": {
      const question = str(raw, "question");
      if (question === undefined || !Array.isArray(raw.answers)) return undefined;
      return { ...base, type: "askMono", question, answers: raw.answers.filter(isString) };
    }
    case "if":
      return {
        ...base,
        type: "if",
        ...opt("condition", str(raw, "condition")),
        ...opt("decide", raw.decide === "mono" ? ("mono" as const) : undefined),
        ...opt("question", str(raw, "question")),
      };
    case "forEach": {
      const list = str(raw, "in");
      if (list === undefined) return undefined;
      return {
        ...base,
        type: "forEach",
        in: list,
        ...opt("as", str(raw, "as")),
        ...opt("maxParallel", num(raw, "maxParallel")),
      };
    }
    case "report":
      return { ...base, type: "report", ...opt("template", str(raw, "template")) };
  }
}

export function parseEdge(raw: unknown): WorkflowEdge | undefined {
  if (!isRec(raw)) return undefined;
  const from = str(raw, "from");
  const to = str(raw, "to");
  if (!from || !to) return undefined;
  let loop: WorkflowEdge["loop"];
  if (raw.loop !== undefined) {
    if (!isRec(raw.loop) || !isNumber(raw.loop.maxRounds) || !isString(raw.loop.decide)) {
      return undefined;
    }
    loop = { maxRounds: raw.loop.maxRounds, decide: raw.loop.decide };
  }
  return {
    from,
    to,
    ...(raw.handle === "true" || raw.handle === "false" ? { handle: raw.handle } : {}),
    ...opt("loop", loop),
  };
}

export function parseInputs(raw: unknown): Record<string, WorkflowInput> {
  const inputs: Record<string, WorkflowInput> = {};
  if (!isRec(raw)) return inputs;
  for (const [name, input] of Object.entries(raw)) {
    if (!isMapKey(name) || !isRec(input) || !INPUT_TYPES.includes(input.type as WorkflowInput["type"])) continue;
    inputs[name] = {
      type: input.type as WorkflowInput["type"],
      ...opt("description", str(input, "description")),
    };
  }
  return inputs;
}

function parseInputValues(raw: unknown): Record<string, string | number | boolean> {
  const values: Record<string, string | number | boolean> = {};
  if (!isRec(raw)) return values;
  for (const [name, value] of Object.entries(raw)) {
    if (!isMapKey(name)) continue;
    if (isString(value) || isNumber(value) || isBool(value)) values[name] = value;
  }
  return values;
}

function parseWorkflow(raw: unknown): Workflow | undefined {
  if (!isRec(raw)) return undefined;
  const id = str(raw, "id");
  if (!id || !Array.isArray(raw.nodes)) return undefined;
  const nodes = raw.nodes.map(parseNode);
  const rawEdges = Array.isArray(raw.edges) ? raw.edges : [];
  const edges = rawEdges.map(parseEdge);
  // A node or edge that cannot be read could not run; dropping it quietly would change what
  // the workflow does, so the whole entry is left out instead.
  if (nodes.some((n) => !n) || edges.some((e) => !e)) return undefined;
  return {
    id,
    name: str(raw, "name") ?? "",
    ...opt("description", str(raw, "description")),
    inputs: parseInputs(raw.inputs),
    inputValues: parseInputValues(raw.inputValues),
    nodes: nodes as WorkflowNode[],
    edges: edges as WorkflowEdge[],
    // An entry that says nothing about being enabled stays paused.
    enabled: raw.enabled === true,
    createdAt: num(raw, "createdAt") ?? 0,
    ...opt("nextRunAt", num(raw, "nextRunAt")),
    ...(raw.runRequested === true ? { runRequested: true } : {}),
    ...(raw.stopRequested === true ? { stopRequested: true } : {}),
    ...opt("resumeRunId", str(raw, "resumeRunId")),
    ...opt("lastRunId", str(raw, "lastRunId")),
    ...opt("importedFrom", str(raw, "importedFrom")),
    ...(raw.needsReview === true ? { needsReview: true } : {}),
  };
}

/** What workflows.json holds: the entries that parse, and the ones that do not, as they were written. */
export type WorkflowFile = { workflows: Workflow[]; unreadable: unknown[] };

/**
 * Splits workflows.json into readable and unreadable entries. An entry is unreadable when it does
 * not parse as a workflow (a node of a type this app does not know, a damaged field) or repeats an
 * earlier id. Undefined when the text is not a JSON list, so a caller can refuse to overwrite it.
 * A blank text is an empty file. Never throws.
 */
export function readWorkflowFile(text: string): WorkflowFile | undefined {
  if (!text?.trim()) return { workflows: [], unreadable: [] };
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return undefined;
  }
  if (!Array.isArray(parsed)) return undefined;
  const workflows: Workflow[] = [];
  const unreadable: unknown[] = [];
  const seen = new Set<string>();
  for (const raw of parsed) {
    const workflow = parseWorkflow(raw);
    if (!workflow || seen.has(workflow.id)) {
      unreadable.push(raw);
      continue;
    }
    seen.add(workflow.id);
    workflows.push(workflow);
  }
  return { workflows, unreadable };
}

/** workflows.json as written by the app; anything malformed is left out. Never throws. */
export function parseWorkflows(text: string): Workflow[] {
  return readWorkflowFile(text)?.workflows ?? [];
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

const PROJECT_REF = /^\{\{inputs\.([^{}\s.]+)\}\}$/;
const RUN_VARS = ["round", "id", "startedAt"];
/** Template variables a node output must not shadow. */
const RESERVED_NODE_IDS = ["run", "inputs", "results"];
/** Extra `run` fields only a report template gets (see composeReport). */
const REPORT_RUN_VARS = ["name", "status", "statusLabel", "endedAt"];
/**
 * Something that reads as a path on this machine: a home or well-known root directory, a Windows
 * drive, or `~/`. A bare `/api/users` is not matched, so prompts may talk about URL paths.
 */
const ABSOLUTE_PATH =
  /(?:^|[\s"'`=(\[,<>|;&:])(?:~\/|\/(?:Users|home|Volumes|private|tmp|var|opt|etc|usr|mnt|root|Library|Applications|System)\/|[A-Za-z]:[\\/])\S*/;
/**
 * For shell commands every word that starts with `/` or `~/` is a path on this machine, also
 * right after a redirect or operator (`>/tmp/log`, `;/usr/bin/x`). `//` is left alone so a URL
 * such as https://host/a/b is not read as a path, and so are the harmless /dev/null-style
 * device files used to discard output.
 */
const COMMAND_ABSOLUTE_PATH =
  /(?:^|[\s"'`=(\[,<>|;&:])(?:~\/|\/(?!\/|dev\/(?:null|stdin|stdout|stderr)(?![\w/.-]))\S|[A-Za-z]:[\\/])\S*/;

/** The first thing in `text` that reads as an absolute path on this machine, if any. */
export function findAbsolutePath(text: string): string | undefined {
  return ABSOLUTE_PATH.exec(text)?.[0].trim();
}

const COMMAND_FIELDS = ["exitCode", "stdout", "stderr"];
const ASK_FIELDS = ["answer", "reason"];
const FOR_EACH_FIELDS = ["results"];

function outputFields(node: WorkflowNode): string[] {
  switch (node.type) {
    case "session":
      return Object.keys(node.output);
    case "command":
      return COMMAND_FIELDS;
    case "askMono":
      return ASK_FIELDS;
    case "forEach":
      return FOR_EACH_FIELDS;
    default:
      return [];
  }
}

type Graph = {
  byId: Map<NodeId, WorkflowNode>;
  /** Neighbours over non-loop edges whose endpoints both exist. */
  next: Map<NodeId, NodeId[]>;
  prev: Map<NodeId, NodeId[]>;
};

function reach(start: NodeId, adjacency: Map<NodeId, NodeId[]>): Set<NodeId> {
  const seen = new Set<NodeId>();
  const queue = [start];
  while (queue.length) {
    for (const n of adjacency.get(queue.pop() as NodeId) ?? []) {
      if (!seen.has(n)) {
        seen.add(n);
        queue.push(n);
      }
    }
  }
  return seen;
}

function buildGraph(w: Workflow): Graph {
  const byId = new Map<NodeId, WorkflowNode>();
  for (const node of w.nodes) if (!byId.has(node.id)) byId.set(node.id, node);
  const next = new Map<NodeId, NodeId[]>();
  const prev = new Map<NodeId, NodeId[]>();
  for (const e of w.edges) {
    if (e.loop || !byId.has(e.from) || !byId.has(e.to)) continue;
    next.set(e.from, [...(next.get(e.from) ?? []), e.to]);
    prev.set(e.to, [...(prev.get(e.to) ?? []), e.from]);
  }
  return { byId, next, prev };
}

/** Nodes that sit on a cycle of non-loop edges (or are reachable only through one). */
function cycleNodes(g: Graph): NodeId[] {
  const indegree = new Map<NodeId, number>();
  for (const id of g.byId.keys()) indegree.set(id, g.prev.get(id)?.length ?? 0);
  const queue = [...g.byId.keys()].filter((id) => indegree.get(id) === 0);
  const done = new Set<NodeId>();
  while (queue.length) {
    const id = queue.pop() as NodeId;
    done.add(id);
    for (const n of g.next.get(id) ?? []) {
      const left = (indegree.get(n) ?? 0) - 1;
      indegree.set(n, left);
      if (left === 0) queue.push(n);
    }
  }
  return [...g.byId.keys()].filter((id) => !done.has(id));
}

export function validateWorkflow(w: Workflow): WorkflowIssue[] {
  const issues: WorkflowIssue[] = [];
  const add = (message: string, nodeId?: NodeId) =>
    issues.push(nodeId === undefined ? { message } : { nodeId, message });

  const g = buildGraph(w);
  const ancestorsOf = new Map<NodeId, Set<NodeId>>();
  const descendantsOf = new Map<NodeId, Set<NodeId>>();
  const ancestors = (id: NodeId) => {
    let set = ancestorsOf.get(id);
    if (!set) ancestorsOf.set(id, (set = reach(id, g.prev)));
    return set;
  };
  const descendants = (id: NodeId) => {
    let set = descendantsOf.get(id);
    if (!set) descendantsOf.set(id, (set = reach(id, g.next)));
    return set;
  };

  // --- structure -------------------------------------------------------------------------
  const triggers = w.nodes.filter(isTrigger);
  if (triggers.length !== 1) {
    add(`A workflow needs exactly one trigger (found ${triggers.length})`);
  }
  const reports = w.nodes.filter((n) => n.type === "report");
  if (reports.length !== 1) {
    add(`A workflow needs exactly one Report node (found ${reports.length})`);
  } else if (triggers.length === 1) {
    const reachable = reach(triggers[0].id, g.next);
    if (!reachable.has(reports[0].id)) {
      add("The Report node must be reachable from the trigger", reports[0].id);
    }
  }

  const ids = new Set<NodeId>();
  for (const node of w.nodes) {
    if (!NODE_ID.test(node.id)) {
      add(
        `Invalid node id "${node.id}": use lowercase letters, digits and underscores, starting with a letter (max 40)`,
        node.id,
      );
    }
    if (RESERVED_NODE_IDS.includes(node.id)) {
      add(`Node id "${node.id}" is reserved (it is a template variable)`, node.id);
    }
    if (ids.has(node.id)) add(`Duplicate node id "${node.id}"`, node.id);
    ids.add(node.id);
  }

  for (const name of Object.keys(w.inputs)) {
    if (!isMapKey(name)) {
      add(`Invalid input name "${name}": use letters, digits and underscores`);
    }
  }

  for (const e of w.edges) {
    for (const end of [e.from, e.to]) {
      if (!g.byId.has(end)) add(`Edge ${e.from} -> ${e.to} points at "${end}", which does not exist`);
    }
  }

  const cyclic = cycleNodes(g);
  if (cyclic.length) {
    add(
      `The workflow has a cycle without a loop edge (${cyclic.join(", ")}); declare a loop edge to repeat steps`,
    );
  }

  // --- per node, in the order the user reads them ----------------------------------------
  const parentChain = (node: WorkflowNode): ForEachNodeLike[] => {
    const chain: ForEachNodeLike[] = [];
    const seen = new Set<NodeId>([node.id]);
    let parent = node.parent ? g.byId.get(node.parent) : undefined;
    while (parent && parent.type === "forEach" && !seen.has(parent.id)) {
      chain.push(parent);
      seen.add(parent.id);
      parent = parent.parent ? g.byId.get(parent.parent) : undefined;
    }
    return chain;
  };

  // Nodes whose output is in scope for `id`: upstream over non-loop edges, the loop body when
  // inside a loop (it holds the previous round's output), and whatever is upstream of the
  // enclosing For each nodes.
  const scopeOf = (id: NodeId): Set<NodeId> => {
    const scope = new Set<NodeId>();
    const visit = (nodeId: NodeId, seen: Set<NodeId>) => {
      if (seen.has(nodeId)) return;
      seen.add(nodeId);
      for (const a of ancestors(nodeId)) scope.add(a);
      for (const e of w.edges) {
        if (!e.loop || !g.byId.has(e.from) || !g.byId.has(e.to)) continue;
        const body = [...descendants(e.to), e.to].filter(
          (n) => n === e.from || ancestors(e.from).has(n),
        );
        if (!body.includes(nodeId)) continue;
        for (const n of body) if (n !== nodeId) scope.add(n);
      }
      const parent = g.byId.get(nodeId)?.parent;
      if (parent && g.byId.get(parent)?.type === "forEach") visit(parent, seen);
    };
    visit(id, new Set());
    scope.delete(id);
    return scope;
  };

  const checkVars = (
    text: string,
    nodeId: NodeId | undefined,
    scope: Set<NodeId>,
    itemNames: Set<string>,
  ) => {
    checkVarList(templateVars(text), nodeId, scope, itemNames);
  };

  const checkVarList = (
    vars: string[],
    nodeId: NodeId | undefined,
    scope: Set<NodeId>,
    itemNames: Set<string>,
    runVars: string[] = RUN_VARS,
  ) => {
    for (const v of vars) {
      const [head, field] = v.split(".");
      const path = v.split(".");
      const label = `{{${v}}}`;
      if (head === "inputs") {
        if (path.length < 2 || !Object.prototype.hasOwnProperty.call(w.inputs, field)) {
          add(`${label} does not name a declared input`, nodeId);
        }
      } else if (head === "run") {
        if (path.length !== 2 || !runVars.includes(field)) {
          add(
            `${label} is not a run variable (use ${runVars.map((f) => `run.${f}`).join(", ")})`,
            nodeId,
          );
        }
      } else if (itemNames.has(head)) {
        // Any path under the loop variable is allowed; the items' shape is not declared.
      } else {
        const node = g.byId.get(head);
        if (!node) {
          add(`${label} is unknown: it names no input, upstream node or loop variable`, nodeId);
        } else if (!scope.has(head)) {
          add(`${label} refers to "${head}", which is not upstream of this node`, nodeId);
        } else if (path.length > 1 && !outputFields(node).includes(field)) {
          add(`${label}: "${head}" has no output field "${field}"`, nodeId);
        }
      }
    }
  };

  const checkText = (
    text: string,
    node: WorkflowNode,
    extraScope: Iterable<NodeId> = [],
  ) => {
    const scope = scopeOf(node.id);
    for (const n of extraScope) scope.add(n);
    const itemNames = new Set(parentChain(node).map((p) => p.as ?? "item"));
    checkVars(text, node.id, scope, itemNames);
  };

  const checkPaths = (node: WorkflowNode) => {
    const walk = (value: unknown, label: string, strict = false) => {
      if (isString(value)) {
        const m = (strict ? COMMAND_ABSOLUTE_PATH : ABSOLUTE_PATH).exec(value);
        if (m) {
          add(
            `${label} contains an absolute path ("${m[0].trim()}"); use a project input or a relative path`,
            node.id,
          );
        }
      } else if (Array.isArray(value)) {
        value.forEach((item) => walk(item, label));
      } else if (isRec(value)) {
        for (const [key, item] of Object.entries(value)) walk(item, `${label}.${key}`);
      }
    };
    for (const [key, value] of Object.entries(node)) {
      if (["id", "type", "parent", "ui", "project"].includes(key)) continue;
      walk(value, key, node.type === "command" && key === "command");
    }
  };

  const checkProject = (node: WorkflowNode & { project: string }) => {
    const m = PROJECT_REF.exec(node.project);
    if (!m) {
      add(
        ABSOLUTE_PATH.test(node.project) || node.project.startsWith("/")
          ? "project is an absolute path; it must be {{inputs.<name>}} naming a project input"
          : "project must be exactly {{inputs.<name>}} naming a project input",
        node.id,
      );
    } else if (
      !Object.prototype.hasOwnProperty.call(w.inputs, m[1]) ||
      w.inputs[m[1]].type !== "project"
    ) {
      add(`${node.project} is not a declared project input`, node.id);
    }
  };

  const checkSessionRef = (ref: NodeId, what: string, node: WorkflowNode) => {
    const target = g.byId.get(ref);
    if (!target) add(`${what} "${ref}" does not exist`, node.id);
    else if (target.type !== "session") add(`${what} "${ref}" must be a session node`, node.id);
  };

  const checkTimeout = (node: WorkflowNode & { timeoutMinutes?: number }) => {
    if (node.timeoutMinutes !== undefined && !(node.timeoutMinutes > 0)) {
      add("timeoutMinutes must be greater than 0", node.id);
    } else if (node.timeoutMinutes !== undefined && node.timeoutMinutes > MAX_TIMEOUT_MINUTES) {
      add(`timeoutMinutes must be at most ${MAX_TIMEOUT_MINUTES} (24 hours)`, node.id);
    }
  };

  for (const node of w.nodes) {
    if (node.parent !== undefined) {
      const parent = g.byId.get(node.parent);
      if (!parent) add(`parent "${node.parent}" does not exist`, node.id);
      else if (parent.type !== "forEach") {
        add(`parent "${node.parent}" must be a For each node`, node.id);
      }
    }
    checkPaths(node);

    switch (node.type) {
      case "schedule":
        try {
          const s = node.schedule;
          habitSchedule({
            kind: s.scheduleKind,
            time: s.time,
            minute: s.minute,
            dayOfWeek: s.dayOfWeek,
            ...(s.intervalHours !== undefined ? { hours: s.intervalHours } : {}),
          });
        } catch (err) {
          add(`Invalid schedule: ${err instanceof Error ? err.message : String(err)}`, node.id);
        }
        break;
      case "manual":
        break;
      case "session": {
        checkProject(node);
        checkText(node.prompt, node);
        for (const name of Object.keys(node.output)) {
          if (!isMapKey(name)) {
            add(`Invalid output field name "${name}": use letters, digits and underscores`, node.id);
          }
        }
        if (typeof node.checkout === "object") {
          checkSessionRef(node.checkout.worktreeOf, "checkout worktreeOf", node);
        }
        if (node.nestUnder !== undefined) checkSessionRef(node.nestUnder, "nestUnder", node);
        checkTimeout(node);
        break;
      }
      case "command": {
        checkProject(node);
        checkText(node.command, node);
        // Commands are shell text, so quoting problems are security problems.
        for (const problem of shellTemplateProblems(node.command)) add(problem, node.id);
        if (typeof node.cwd === "object") {
          checkSessionRef(node.cwd.worktreeOf, "cwd worktreeOf", node);
        }
        checkTimeout(node);
        break;
      }
      case "askMono":
        checkText(node.question, node);
        if (node.answers.length === 0) add("An Ask Mono node needs at least one answer", node.id);
        break;
      case "if": {
        const hasCondition = !!node.condition?.trim();
        const decidesByMono = node.decide === "mono";
        if (hasCondition === decidesByMono) {
          add('An If node needs either a condition or decide "mono", not both or neither', node.id);
        }
        if (node.condition) checkText(node.condition, node);
        if (node.question) checkText(node.question, node);
        break;
      }
      case "forEach": {
        if (!node.in.trim()) add("A For each node needs an input list", node.id);
        else checkText(node.in, node);
        const as = node.as ?? "item";
        if (!NAME.test(as) || RESERVED_NODE_IDS.includes(as) || g.byId.has(as)) {
          add(`Invalid loop variable name "${as}"`, node.id);
        }
        if (
          node.maxParallel !== undefined &&
          !(Number.isInteger(node.maxParallel) && node.maxParallel >= 1 && node.maxParallel <= 10)
        ) {
          add("maxParallel must be a whole number from 1 to 10", node.id);
        }
        break;
      }
      case "report": {
        if (node.parent !== undefined) add("The Report node cannot be inside a For each", node.id);
        if (node.template) {
          // The report is rendered with `inputs`, `run`, `results` and the outputs of top-level
          // nodes (see composeReport). Steps inside a For each, and its loop variable, are not in
          // that scope: their outputs reach the report through `results`.
          const loopNames = new Set(
            w.nodes.filter((n) => n.type === "forEach").map((n) => (n.type === "forEach" ? n.as ?? "item" : "")),
          );
          const readable: string[] = [];
          for (const v of templateVars(node.template)) {
            const head = v.split(".")[0];
            if (head === "results") continue;
            const target = g.byId.get(head);
            if (target?.parent !== undefined) {
              add(
                `The report cannot read "${v}": steps inside a For each are listed in {{results}} (or use {{${target.parent}.results}}).`,
                node.id,
              );
            } else if (!target && head !== "inputs" && head !== "run" && loopNames.has(head)) {
              add(
                `The report cannot read "${v}": the loop variable only exists inside a For each; use {{results}}.`,
                node.id,
              );
            } else {
              readable.push(v);
            }
          }
          checkVarList(readable, node.id, scopeOf(node.id), new Set(), [
            ...RUN_VARS,
            ...REPORT_RUN_VARS,
          ]);
        }
        break;
      }
    }
  }

  // --- edges -----------------------------------------------------------------------------
  const loopSources = new Set<NodeId>();
  for (const e of w.edges) {
    const from = g.byId.get(e.from);
    const to = g.byId.get(e.to);
    if (e.handle !== undefined && from && from.type !== "if") {
      add(`Edge ${e.from} -> ${e.to} has a handle, but only If nodes have true/false handles`, e.from);
    }
    if (from?.type === "if" && !e.loop && e.handle !== "true" && e.handle !== "false") {
      add(`Edge ${e.from} -> ${e.to} leaves an If node without a "true" or "false" handle`, e.from);
    }
    if (from?.type === "report" && !e.loop) {
      add(`Edge ${e.from} -> ${e.to} leaves the Report node; the Report node must be the end`, e.from);
    }
    if (!e.loop || !from || !to) continue;

    if (from.type !== "session" && from.type !== "command" && from.type !== "askMono") {
      add(
        `Loop edge ${e.from} -> ${e.to}: a loop can only start from a session, command or Ask Mono node`,
        e.from,
      );
    }
    if (w.edges.filter((o) => o.loop && o.from === e.from).length > 1 && !loopSources.has(e.from)) {
      loopSources.add(e.from);
      add(`Node "${e.from}" is the source of more than one loop edge; only one is allowed`, e.from);
    }
    if (!ancestors(e.from).has(e.to)) {
      add(`Loop edge ${e.from} -> ${e.to}: "${e.to}" must be an ancestor of "${e.from}"`, e.from);
    }
    if (from.parent !== to.parent) {
      add(`Loop edge ${e.from} -> ${e.to} must stay inside the same For each`, e.from);
    }
    const { maxRounds, decide } = e.loop;
    if (!(Number.isInteger(maxRounds) && maxRounds >= 1 && maxRounds <= 10)) {
      add(`Loop edge ${e.from} -> ${e.to}: maxRounds must be a whole number from 1 to 10`, e.from);
    }
    if (decide !== "mono") {
      if (!decide.trim()) {
        add(`Loop edge ${e.from} -> ${e.to}: decide must be "mono" or a condition`, e.from);
      } else {
        const scope = scopeOf(e.from);
        scope.add(e.from);
        const itemNames = new Set(parentChain(from).map((p) => p.as ?? "item"));
        checkVars(decide, e.from, scope, itemNames);
      }
    }
  }

  return issues;
}

type ForEachNodeLike = Extract<WorkflowNode, { type: "forEach" }>;
