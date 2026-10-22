import { Scalar, parse, stringify } from "yaml";
import { anchorSchedule, type HabitSchedule } from "../../monos/model/monoHabits";
import {
  NODE_TYPES,
  newWorkflowId,
  type NodeId,
  type NodeType,
  type OutputField,
  type Workflow,
  type WorkflowEdge,
  type WorkflowInput,
  type WorkflowNode,
} from "./workflow";
import { outputInstruction } from "./workflowOutput";
import {
  findAbsolutePath,
  parseEdge,
  parseInputs,
  parseNode,
  validateWorkflow,
  type WorkflowIssue,
} from "./workflowParse";

/** The value of the top-level `monoptahWorkflow` key. */
export const WORKFLOW_FILE_VERSION = 1;
/** Larger files are refused before they are parsed. */
export const MAX_FILE_BYTES = 512 * 1024;
/** Caps alias expansion, so a file cannot blow up into a huge structure. */
const MAX_ALIAS_COUNT = 20;

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => !!v && typeof v === "object" && !Array.isArray(v);

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

/** A copy of `obj` without its undefined values; key order is kept. */
function defined<T extends Rec>(obj: T): T {
  return Object.fromEntries(Object.entries(obj).filter(([, v]) => v !== undefined)) as T;
}

/**
 * The file writes a schedule the way the spec does, `{ kind, time, ... }`; the app stores it as
 * a HabitSchedule. Fields a schedule keeps at their defaults are left out.
 */
function exportSchedule(s: HabitSchedule): Rec {
  // Quoted, because a YAML 1.1 reader takes a bare 09:00 for a number.
  const time = new Scalar(s.time);
  time.type = Scalar.QUOTE_DOUBLE;
  return defined({
    kind: s.scheduleKind,
    time,
    minute: s.minute !== 0 ? s.minute : undefined,
    dayOfWeek: s.dayOfWeek !== 1 ? s.dayOfWeek : undefined,
    hours: s.intervalHours,
    intervalFrom: s.intervalFrom,
  });
}

function importSchedule(raw: unknown): unknown {
  if (!isRec(raw) || raw.scheduleKind !== undefined || raw.kind === undefined) return raw;
  const { kind, hours, ...rest } = raw;
  return defined({ ...rest, scheduleKind: kind, intervalHours: hours ?? rest.intervalHours });
}

function exportOutput(output: Record<string, OutputField>): Rec {
  return Object.fromEntries(
    Object.entries(output).map(([name, f]) => [
      name,
      defined({ type: f.type, inReport: f.inReport, description: f.description }),
    ]),
  );
}

function exportNode(node: WorkflowNode): Rec {
  const head = { id: node.id, type: node.type, title: node.title, parent: node.parent };
  const ui = node.ui ? { x: node.ui.x, y: node.ui.y } : undefined;
  switch (node.type) {
    case "schedule":
      return defined({ ...head, schedule: exportSchedule(node.schedule), ui });
    case "manual":
      return defined({ ...head, ui });
    case "session":
      return defined({
        ...head,
        project: node.project,
        harness: node.harness,
        model: node.model,
        effort: node.effort,
        checkout: node.checkout,
        nestUnder: node.nestUnder,
        reuseSession: node.reuseSession,
        timeoutMinutes: node.timeoutMinutes,
        prompt: node.prompt,
        output: exportOutput(node.output),
        ui,
      });
    case "command":
      return defined({
        ...head,
        project: node.project,
        cwd: node.cwd,
        timeoutMinutes: node.timeoutMinutes,
        command: node.command,
        ui,
      });
    case "askMono":
      return defined({ ...head, question: node.question, answers: [...node.answers], ui });
    case "if":
      return defined({
        ...head,
        condition: node.condition,
        decide: node.decide,
        question: node.question,
        ui,
      });
    case "forEach":
      return defined({ ...head, in: node.in, as: node.as, maxParallel: node.maxParallel, ui });
    case "report":
      return defined({ ...head, template: node.template, ui });
  }
}

function exportEdge(edge: WorkflowEdge): Rec {
  return defined({
    from: edge.from,
    to: edge.to,
    handle: edge.handle,
    loop: edge.loop ? { maxRounds: edge.loop.maxRounds, decide: edge.loop.decide } : undefined,
  });
}

function exportInput(input: WorkflowInput): Rec {
  return defined({ type: input.type, description: input.description });
}

/**
 * The shareable definition as YAML: only name, description, inputs (declarations, no values),
 * nodes (with board positions) and edges. The output is deterministic.
 */
function exportDocument(w: Workflow): Rec {
  return defined({
    monoptahWorkflow: WORKFLOW_FILE_VERSION,
    name: w.name,
    description: w.description,
    inputs: Object.fromEntries(Object.entries(w.inputs).map(([k, v]) => [k, exportInput(v)])),
    nodes: w.nodes.map(exportNode),
    edges: w.edges.map(exportEdge),
  });
}

export function exportWorkflow(w: Workflow): string {
  // lineWidth 0 keeps long prompts on one line instead of folding them.
  return stringify(exportDocument(w), { lineWidth: 0 });
}

/**
 * Why this workflow cannot be shared, when a string in it (name, description, input
 * descriptions, prompts, commands...) reads as an absolute path: the same rule previewImport
 * applies, so a file the app writes is a file it can read back. Undefined when it is fine.
 */
export function exportPathProblem(w: Workflow): string | undefined {
  const problem = findPathField(exportDocument(w), "");
  return problem ? `${problem}; use a project input or a relative path` : undefined;
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

export type ImportPreview = {
  workflow: Omit<Workflow, "id" | "createdAt" | "inputValues" | "enabled">;
  /** Every command node's command, verbatim, for the user to review. */
  commands: { nodeId: NodeId; command: string }[];
  /**
   * Every text that reaches an agent, as the agent will read it before templates are filled in:
   * a session's prompt followed by the output instruction (which carries the output field
   * descriptions), the questions put to the Mono with their answers, and a report's template
   * (the report is posted into the Mono's own session, so the Mono reads it as its own reply).
   */
  prompts: { nodeId: NodeId; text: string }[];
  issues: WorkflowIssue[];
};

type Parsed = { ok: true; value: unknown } | { ok: false; error: string };

function parseFile(text: string): Parsed {
  if (new TextEncoder().encode(text).length > MAX_FILE_BYTES) {
    return { ok: false, error: "Workflow file is too large (over 512 KiB)" };
  }
  if (!text.trim()) return { ok: false, error: "Workflow file is empty" };
  if (text.trimStart().startsWith("{")) {
    try {
      return { ok: true, value: JSON.parse(text) };
    } catch {
      // Not JSON after all; the YAML parser reports what is wrong.
    }
  }
  try {
    // The core schema has no custom tags and no merge keys; "error" keeps warnings quiet.
    return {
      ok: true,
      value: parse(text, { schema: "core", maxAliasCount: MAX_ALIAS_COUNT, logLevel: "error" }),
    };
  } catch (err) {
    const message = err instanceof Error ? err.message.split("\n")[0] : String(err);
    return { ok: false, error: `Could not read the workflow file: ${message}` };
  }
}

/** The first string anywhere in `value` that reads as an absolute path, with where it sits. */
function findPathField(value: unknown, label: string): string | undefined {
  if (value instanceof Scalar) return findPathField(value.value, label);
  if (typeof value === "string") {
    const found = findAbsolutePath(value);
    return found ? `${label} contains an absolute path ("${found}")` : undefined;
  }
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      const hit = findPathField(value[i], `${label}[${i}]`);
      if (hit) return hit;
    }
  } else if (isRec(value)) {
    for (const [key, item] of Object.entries(value)) {
      const hit = findPathField(item, label ? `${label}.${key}` : key);
      if (hit) return hit;
    }
  }
  return undefined;
}

function describeVersion(v: unknown): string {
  return typeof v === "number" || typeof v === "string" ? String(v) : "missing";
}

/** Reads YAML or JSON. Validation problems are reported in `issues`, not as a rejection. */
export function previewImport(
  text: string,
): { ok: true; preview: ImportPreview } | { ok: false; error: string } {
  const fail = (error: string) => ({ ok: false as const, error });
  const parsed = parseFile(text);
  if (!parsed.ok) return fail(parsed.error);
  const file = parsed.value;
  if (!isRec(file)) return fail("Not a Monoptah workflow file");

  if (file.monoptahWorkflow !== WORKFLOW_FILE_VERSION) {
    return fail(
      `Unsupported workflow file version: ${describeVersion(file.monoptahWorkflow)} ` +
        `(this app reads version ${WORKFLOW_FILE_VERSION})`,
    );
  }
  const pathProblem = findPathField(file, "");
  if (pathProblem) {
    return fail(`${pathProblem}; use a project input or a relative path`);
  }

  if (typeof file.name !== "string" || !file.name.trim()) {
    return fail("The workflow file needs a name");
  }
  if (file.description !== undefined && typeof file.description !== "string") {
    return fail("description must be text");
  }
  if (!Array.isArray(file.nodes)) return fail("The workflow file needs a list of nodes");

  const nodes: WorkflowNode[] = [];
  for (const [index, raw] of file.nodes.entries()) {
    const id = isRec(raw) && typeof raw.id === "string" ? raw.id : undefined;
    if (!isRec(raw) || !id) return fail(`nodes[${index}] needs an id`);
    if (typeof raw.type !== "string" || !NODE_TYPES.includes(raw.type as NodeType)) {
      return fail(`Node "${id}" has an unknown type: ${JSON.stringify(raw.type) ?? "missing"}`);
    }
    const node = parseNode(
      raw.type === "schedule" ? { ...raw, schedule: importSchedule(raw.schedule) } : raw,
    );
    if (!node) return fail(`Node "${id}" is missing a required field or has one of the wrong type`);
    nodes.push(node);
  }

  const rawEdges = file.edges === undefined ? [] : file.edges;
  if (!Array.isArray(rawEdges)) return fail("edges must be a list");
  const edges: WorkflowEdge[] = [];
  for (const [index, raw] of rawEdges.entries()) {
    const edge = parseEdge(raw);
    if (!edge) return fail(`edge ${index + 1} needs "from" and "to" (and a valid loop, if any)`);
    edges.push(edge);
  }

  const workflow: ImportPreview["workflow"] = {
    name: file.name,
    ...(typeof file.description === "string" ? { description: file.description } : {}),
    inputs: parseInputs(file.inputs),
    nodes,
    edges,
  };
  const issues = validateWorkflow({
    ...workflow,
    id: "wf-preview",
    createdAt: 0,
    inputValues: {},
    enabled: false,
  });

  const commands: ImportPreview["commands"] = [];
  const prompts: ImportPreview["prompts"] = [];
  for (const node of nodes) {
    if (node.type === "command") commands.push({ nodeId: node.id, command: node.command });
    else if (node.type === "session") {
      const instruction = outputInstruction(node.output);
      prompts.push({
        nodeId: node.id,
        text: instruction ? `${node.prompt}\n\n${instruction}` : node.prompt,
      });
    } else if (node.type === "askMono") {
      const answers = node.answers.map((a) => `- ${a}`).join("\n");
      prompts.push({
        nodeId: node.id,
        text: answers ? `${node.question}\n\nAnswers:\n${answers}` : node.question,
      });
    } else if (node.type === "if" && node.question !== undefined) {
      prompts.push({ nodeId: node.id, text: node.question });
    } else if (node.type === "report" && node.template?.trim()) {
      prompts.push({ nodeId: node.id, text: node.template });
    }
  }
  return { ok: true, preview: { workflow, commands, prompts, issues } };
}

const has = (obj: object, key: string) => Object.prototype.hasOwnProperty.call(obj, key);

/** What is wrong with one input's value, if anything. */
function inputValueProblem(name: string, input: WorkflowInput, value: unknown): string | undefined {
  if (value === undefined) return `Missing a value for input "${name}"`;
  switch (input.type) {
    case "project":
      return typeof value !== "string" || !value.trim() ? `Input "${name}" needs a project` : undefined;
    case "string":
      return typeof value !== "string" ? `Input "${name}" must be text` : undefined;
    case "number":
      return typeof value !== "number" || !Number.isFinite(value)
        ? `Input "${name}" must be a number`
        : undefined;
    case "boolean":
      return typeof value !== "boolean" ? `Input "${name}" must be true or false` : undefined;
  }
}

/**
 * Every way `values` does not fit the declared `inputs`: a declared input with no value or one
 * of the wrong type (an empty project counts), and a value for an input that is not declared.
 * The one rule behind imports, `workflows.setInputs` and validating a workflow before it is enabled.
 */
export function inputValueProblems(
  inputs: Record<string, WorkflowInput>,
  values: Workflow["inputValues"],
): string[] {
  const problems: string[] = [];
  for (const [name, input] of Object.entries(inputs)) {
    const problem = inputValueProblem(name, input, has(values, name) ? values[name] : undefined);
    if (problem) problems.push(problem);
  }
  for (const name of Object.keys(values))
    if (!has(inputs, name)) problems.push(`Unknown input "${name}"; the workflow declares no such input`);
  return problems;
}

/** An interval schedule counts from the day it was created or imported. */
export function anchorNode(node: WorkflowNode, now: number): WorkflowNode {
  return node.type === "schedule" && !node.schedule.intervalFrom
    ? { ...node, schedule: anchorSchedule(node.schedule, now) }
    : node;
}

/**
 * The workflow to save after the user reviewed the preview: a new id, paused, with a value for
 * every declared input. Throws a message naming the input when a value is missing or has the
 * wrong type.
 */
export function finishImport(
  preview: ImportPreview,
  inputValues: Workflow["inputValues"],
  now: number,
): Workflow {
  // Values for inputs the file does not declare are dropped, not an error.
  const values: Workflow["inputValues"] = {};
  for (const name of Object.keys(preview.workflow.inputs))
    if (has(inputValues, name)) values[name] = inputValues[name];
  const [problem] = inputValueProblems(preview.workflow.inputs, values);
  if (problem) throw new Error(problem);
  const { name, description, inputs, nodes, edges } = structuredClone(preview.workflow);
  return {
    id: newWorkflowId(),
    name,
    ...(description !== undefined ? { description } : {}),
    inputs,
    inputValues: values,
    nodes,
    edges,
    enabled: false,
    createdAt: now,
  };
}
