import type { Session } from "../../sessions/model/session";
import {
  anchorSchedule,
  habitSchedule,
  habitScheduleLabel,
  nextHabitRunAt,
  type HabitSchedule,
} from "../../monos/model/monoHabits";
import {
  WORKFLOWS_MAX,
  newWorkflowId,
  triggerOf,
  type Workflow,
  type WorkflowEdge,
  type WorkflowInput,
  type WorkflowNode,
  type WorkflowRun,
} from "../../workflows/model/workflow";
import {
  parseEdge,
  parseInputs,
  parseNode,
  validateWorkflow,
  type WorkflowIssue,
} from "../../workflows/model/workflowParse";
import {
  exportPathProblem,
  exportWorkflow,
  finishImport,
  previewImport,
} from "../../workflows/model/workflowYaml";
import type { AgentAppHost } from "./agentApp";

/** The fields of a workflow definition the CLI accepts; the app owns everything else. */
const DEFINITION_FIELDS = [
  "name",
  "description",
  "inputs",
  "inputValues",
  "nodes",
  "edges",
] as const;
const NAME_MAX = 80;
const DESCRIPTION_MAX = 1_000;
const DEFINITION_MAX_BYTES = 512 * 1024;
const RUNS_DEFAULT = 5;
const RUNS_MAX = 20;

/** Allowed input fields per action. `project` is accepted like on other actions and ignored. */
export const WORKFLOW_FIELDS: ReadonlyMap<string, readonly string[]> = new Map([
  ["workflows.list", ["project"]],
  ["workflows.get", ["id", "project"]],
  ["workflows.add", ["workflow", "yaml", "values", "project"]],
  ["workflows.update", ["id", "workflow", "project"]],
  ["workflows.setInputs", ["id", "values", "project"]],
  ["workflows.run", ["id", "project"]],
  ["workflows.stop", ["id", "project"]],
  ["workflows.pause", ["id", "project"]],
  ["workflows.resume", ["id", "project"]],
  ["workflows.resumeRun", ["id", "runId", "project"]],
  ["workflows.remove", ["id", "project"]],
  ["workflows.export", ["id", "project"]],
  ["workflows.import", ["yaml", "values", "project"]],
  ["workflows.runs", ["id", "limit", "project"]],
]);

type Rec = Record<string, unknown>;
type Values = Workflow["inputValues"];
type Definition = Pick<Workflow, "name" | "inputs" | "nodes" | "edges"> &
  Partial<Pick<Workflow, "description">>;

const isRec = (value: unknown): value is Rec =>
  !!value && typeof value === "object" && !Array.isArray(value);

function text(value: unknown, name: string, max: number): string {
  if (typeof value !== "string" || !value.trim() || value.length > max)
    throw new Error(`${name} must be a non-empty string under ${max} characters`);
  return value.trim();
}

function issuesText(issues: readonly WorkflowIssue[]): string {
  return issues.map((issue) => issue.message).join("; ");
}

/** A schedule node may be written like a habit's schedule ({"kind":"daily"}) or as stored. */
function scheduleNodeInput(raw: Rec): Rec {
  if (!isRec(raw.schedule) || raw.schedule.kind === undefined) return raw;
  try {
    return { ...raw, schedule: habitSchedule(raw.schedule) };
  } catch (error) {
    throw new Error(
      `Node "${String(raw.id)}": ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

function inputValues(value: unknown, name: string): Values {
  if (!isRec(value)) throw new Error(`${name} must be an object of values`);
  const values: Values = {};
  for (const [key, entry] of Object.entries(value)) {
    if (
      typeof entry !== "string" &&
      typeof entry !== "boolean" &&
      !(typeof entry === "number" && Number.isFinite(entry))
    )
      throw new Error(`${name}.${key} must be text, a number or true or false`);
    // A plain object would take "__proto__" as its prototype, not as a key.
    Object.defineProperty(values, key, {
      value: entry,
      enumerable: true,
      writable: true,
      configurable: true,
    });
  }
  return values;
}

function definitionFrom(
  raw: unknown,
  now: number,
  allowed: readonly string[],
): { definition: Definition; values?: Values; enabled?: boolean } {
  if (!isRec(raw)) throw new Error("workflow must be an object");
  const unknown = Object.keys(raw).filter((key) => !allowed.includes(key));
  if (unknown.length)
    throw new Error(
      `Unknown workflow fields: ${unknown.join(", ")}. Allowed: ${allowed.join(", ")}; the id, creation time and run state are managed by the app`,
    );
  if (JSON.stringify(raw).length > DEFINITION_MAX_BYTES)
    throw new Error("The workflow is too large (over 512 KiB)");
  const name = text(raw.name, "workflow.name", NAME_MAX);
  const description =
    raw.description === undefined
      ? undefined
      : text(raw.description, "workflow.description", DESCRIPTION_MAX);
  if (!Array.isArray(raw.nodes) || !raw.nodes.length)
    throw new Error("workflow.nodes must be a list of nodes");
  const nodes: WorkflowNode[] = raw.nodes.map((entry: unknown, index) => {
    const node = parseNode(
      isRec(entry) && entry.type === "schedule" ? scheduleNodeInput(entry) : entry,
    );
    if (!node) {
      const id = isRec(entry) && typeof entry.id === "string" ? entry.id : "";
      throw new Error(
        `nodes[${index}]${id ? ` ("${id}")` : ""} has an unknown type, lacks a required field or has one of the wrong type`,
      );
    }
    // An interval counts from the day it was created.
    return node.type === "schedule" && !node.schedule.intervalFrom
      ? { ...node, schedule: anchorSchedule(node.schedule, now) }
      : node;
  });
  if (raw.edges !== undefined && !Array.isArray(raw.edges))
    throw new Error("workflow.edges must be a list");
  const edges: WorkflowEdge[] = ((raw.edges as unknown[] | undefined) ?? []).map(
    (entry, index) => {
      const edge = parseEdge(entry);
      if (!edge)
        throw new Error(
          `edges[${index}] needs "from" and "to" (and a valid loop with maxRounds and decide, if any)`,
        );
      return edge;
    },
  );
  let inputs: Record<string, WorkflowInput> = {};
  if (raw.inputs !== undefined) {
    if (!isRec(raw.inputs)) throw new Error("workflow.inputs must be an object");
    inputs = parseInputs(raw.inputs);
    const lost = Object.keys(raw.inputs).filter((key) => !(key in inputs));
    if (lost.length)
      throw new Error(
        `Invalid inputs: ${lost.join(", ")} (use letters, digits and underscores, and a type of project, string, number or boolean)`,
      );
  }
  return {
    definition: {
      name,
      ...(description === undefined ? {} : { description }),
      inputs,
      nodes,
      edges,
    },
    ...(raw.inputValues === undefined
      ? {}
      : { values: inputValues(raw.inputValues, "workflow.inputValues") }),
  };
}

/** Problems that keep a workflow from running: its graph, and inputs without a value. */
function problems(w: Workflow): WorkflowIssue[] {
  const missing = Object.keys(w.inputs)
    .filter((name) => !Object.prototype.hasOwnProperty.call(w.inputValues, name))
    .map((name) => ({
      message: `Input "${name}" has no value; set it with workflows.setInputs`,
    }));
  return [...validateWorkflow(w), ...missing];
}

function scheduleOf(w: Workflow): HabitSchedule | undefined {
  const trigger = triggerOf(w);
  return trigger?.type === "schedule" ? trigger.schedule : undefined;
}

/** The workflow with `nextRunAt` right for its enabled state and schedule. */
function scheduled(w: Workflow, now: number): Workflow {
  const { nextRunAt, ...rest } = w;
  const schedule = scheduleOf(w);
  if (!w.enabled || !schedule) return rest;
  return { ...rest, nextRunAt: nextHabitRunAt(schedule, now) };
}

function triggerView(w: Workflow) {
  const trigger = triggerOf(w);
  if (!trigger) return { type: "none" };
  return trigger.type === "schedule"
    ? { type: "schedule", schedule: habitScheduleLabel(trigger.schedule) }
    : { type: "manual" };
}

function runSummary(run: WorkflowRun) {
  return {
    id: run.id,
    status: run.status,
    trigger: run.trigger,
    startedAt: new Date(run.startedAt).toLocaleString(),
    ...(run.endedAt ? { endedAt: new Date(run.endedAt).toLocaleString() } : {}),
    ...(run.error ? { error: run.error } : {}),
  };
}

/** A run with each node's status per branch (a branch is one item of a forEach). */
function runDetail(run: WorkflowRun) {
  return {
    ...runSummary(run),
    nodes: run.nodes.map((node) => ({
      nodeId: node.nodeId,
      branch: node.branch,
      status: node.status,
      round: node.round,
      ...(node.error ? { error: node.error } : {}),
    })),
  };
}

export async function handleWorkflows(
  source: Session,
  action: string,
  input: Rec,
  host: AgentAppHost,
): Promise<unknown> {
  const workflows = host.workflows;
  const monoId = host.isMono(source.id) ? host.monoOf?.(source.id)?.id : undefined;
  if (!workflows || !monoId)
    throw new Error("Only a Mono's own conversation can manage its workflows");
  const now = (host.now?.() ?? new Date()).getTime();
  const id = () => text(input.id, "id", 128);
  const find = (list: Workflow[]) => {
    const found = list.find((entry) => entry.id === id());
    if (!found) throw new Error("No workflow with that id; run workflows.list");
    return found;
  };
  const change = <T>(edit: (list: Workflow[]) => { list: Workflow[]; result: T }) =>
    workflows.update(monoId, (list) => {
      const { list: next, result } = edit(list);
      return { workflows: next, result };
    });
  const replace = (list: Workflow[], next: Workflow) =>
    list.map((entry) => (entry.id === next.id ? next : entry));
  const running = (workflowId: string) => workflows.running(workflowId) !== undefined;

  switch (action) {
    case "workflows.list": {
      const list = await workflows.load(monoId);
      return {
        workflows: await Promise.all(
          list.map(async (w) => {
            const last = (await workflows.runs(monoId, w.id))[0];
            return {
              id: w.id,
              name: w.name,
              enabled: w.enabled,
              trigger: triggerView(w),
              nextRunAt:
                w.enabled && w.nextRunAt !== undefined
                  ? new Date(w.nextRunAt).toLocaleString()
                  : null,
              running: running(w.id),
              lastRun: last
                ? {
                    id: last.id,
                    status: last.status,
                    startedAt: new Date(last.startedAt).toLocaleString(),
                  }
                : null,
            };
          }),
        ),
      };
    }
    case "workflows.get": {
      const w = find(await workflows.load(monoId));
      const runs = await workflows.runs(monoId, w.id);
      return {
        id: w.id,
        name: w.name,
        ...(w.description === undefined ? {} : { description: w.description }),
        enabled: w.enabled,
        trigger: triggerView(w),
        running: running(w.id),
        inputs: w.inputs,
        inputValues: w.inputValues,
        nodes: w.nodes,
        edges: w.edges,
        issues: problems(w),
        runs: runs.slice(0, 5).map(runSummary),
      };
    }
    case "workflows.add": {
      if ((input.workflow === undefined) === (input.yaml === undefined))
        throw new Error("Pass either workflow (an object) or yaml (text), not both");
      let definition: Definition;
      let values: Values = {};
      if (input.workflow !== undefined) {
        if (input.values !== undefined)
          throw new Error("values goes with yaml; put inputValues in the workflow");
        const parsed = definitionFrom(input.workflow, now, DEFINITION_FIELDS);
        definition = parsed.definition;
        values = parsed.values ?? {};
      } else {
        const preview = previewImport(text(input.yaml, "yaml", DEFINITION_MAX_BYTES));
        if (!preview.ok) throw new Error(preview.error);
        definition = preview.preview.workflow;
        if (input.values !== undefined) values = inputValues(input.values, "values");
      }
      return change((list) => {
        if (list.length >= WORKFLOWS_MAX)
          throw new Error(`A Mono can have at most ${WORKFLOWS_MAX} workflows`);
        const draft: Workflow = {
          ...definition,
          id: newWorkflowId(),
          inputValues: values,
          enabled: false,
          createdAt: now,
        };
        const issues = problems(draft);
        const created = scheduled({ ...draft, enabled: !issues.length }, now);
        return {
          list: [...list, created],
          result: { id: created.id, enabled: created.enabled, issues },
        };
      });
    }
    case "workflows.update": {
      const parsed = definitionFrom(input.workflow, now, [
        ...DEFINITION_FIELDS,
        "enabled",
      ]);
      const wanted = (input.workflow as Rec).enabled;
      if (wanted !== undefined && typeof wanted !== "boolean")
        throw new Error("enabled must be true or false");
      return change((list) => {
        const current = find(list);
        // Only the definition is replaced; the id, creation time and run state stay.
        const { description: _old, ...kept } = current;
        const draft: Workflow = {
          ...kept,
          ...parsed.definition,
          inputValues: parsed.values ?? current.inputValues,
          enabled: wanted ?? current.enabled,
        };
        const issues = problems(draft);
        // A workflow that cannot run is paused rather than left to fail on its schedule.
        const enabled = draft.enabled && !issues.length;
        const sameSchedule =
          JSON.stringify(scheduleOf(current)) === JSON.stringify(scheduleOf(draft));
        // A new schedule, or turning it back on, counts from now; otherwise the time stands.
        const next =
          enabled && (!current.enabled || !sameSchedule || current.nextRunAt === undefined)
            ? scheduled({ ...draft, enabled }, now)
            : { ...draft, enabled };
        return {
          list: replace(list, next),
          result: { id: next.id, enabled, issues },
        };
      });
    }
    case "workflows.setInputs": {
      const values = inputValues(input.values, "values");
      return change((list) => {
        const current = find(list);
        const undeclared = Object.keys(values).filter((key) => !(key in current.inputs));
        if (undeclared.length)
          throw new Error(
            `The workflow has no input named ${undeclared.join(", ")}; declared inputs: ${Object.keys(current.inputs).join(", ") || "none"}`,
          );
        for (const [key, value] of Object.entries(values)) {
          const type = current.inputs[key].type;
          const ok =
            type === "number"
              ? typeof value === "number"
              : type === "boolean"
                ? typeof value === "boolean"
                : typeof value === "string" && (type !== "project" || !!value.trim());
          if (!ok)
            throw new Error(
              `Input "${key}" must be ${type === "number" ? "a number" : type === "boolean" ? "true or false" : type === "project" ? "a project path or name" : "text"}`,
            );
        }
        const next = { ...current, inputValues: { ...current.inputValues, ...values } };
        return {
          list: replace(list, next),
          result: { id: next.id, inputValues: next.inputValues, issues: problems(next) },
        };
      });
    }
    case "workflows.run": {
      const current = find(await workflows.load(monoId));
      if (running(current.id))
        throw new Error("That workflow is already running; its report is posted here when it ends");
      const issues = problems(current);
      if (issues.length)
        throw new Error(`The workflow cannot run yet: ${issuesText(issues)}`);
      await change((list) => ({
        list: replace(list, { ...find(list), runRequested: true }),
        result: undefined,
      }));
      workflows.checkNow();
      return {
        id: current.id,
        note: "It starts now, on its own. Its report is posted to this chat when it finishes.",
      };
    }
    case "workflows.stop": {
      const current = find(await workflows.load(monoId));
      if (!running(current.id)) throw new Error("That workflow is not running");
      await workflows.stop(monoId, current.id);
      return { id: current.id, note: "Stop requested; the run ends within seconds." };
    }
    case "workflows.pause":
      return change((list) => {
        const next = { ...find(list), enabled: false };
        return { list: replace(list, next), result: { id: next.id, enabled: false } };
      });
    case "workflows.resume":
      return change((list) => {
        const current = find(list);
        const issues = problems(current);
        if (issues.length)
          throw new Error(`The workflow cannot be resumed yet: ${issuesText(issues)}`);
        const next = scheduled({ ...current, enabled: true }, now);
        return { list: replace(list, next), result: { id: next.id, enabled: true } };
      });
    case "workflows.resumeRun": {
      const runId = text(input.runId, "runId", 128);
      const current = find(await workflows.load(monoId));
      if (running(current.id))
        throw new Error("That workflow is already running");
      const run = (await workflows.runs(monoId, current.id)).find(
        (entry) => entry.id === runId,
      );
      if (!run) throw new Error("No run with that id; run workflows.runs");
      if (run.status !== "interrupted")
        throw new Error(`Only an interrupted run can be resumed; this one is ${run.status}`);
      await workflows.resumeRun(monoId, current.id, run.id);
      return { id: current.id, runId: run.id, note: "It continues from where it stopped." };
    }
    case "workflows.remove": {
      const current = find(await workflows.load(monoId));
      if (running(current.id)) throw new Error("Stop the workflow before removing it");
      await change((list) => ({
        list: list.filter((entry) => entry.id !== current.id),
        result: undefined,
      }));
      return { removed: current.name };
    }
    case "workflows.export": {
      const current = find(await workflows.load(monoId));
      const problem = exportPathProblem(current);
      if (problem) throw new Error(`This workflow cannot be exported: ${problem}`);
      return { yaml: exportWorkflow(current) };
    }
    case "workflows.import": {
      const preview = previewImport(text(input.yaml, "yaml", DEFINITION_MAX_BYTES));
      if (!preview.ok) throw new Error(preview.error);
      const values =
        input.values === undefined ? {} : inputValues(input.values, "values");
      const created = finishImport(preview.preview, values, now);
      await change((list) => {
        if (list.length >= WORKFLOWS_MAX)
          throw new Error(`A Mono can have at most ${WORKFLOWS_MAX} workflows`);
        return { list: [...list, created], result: undefined };
      });
      return {
        id: created.id,
        name: created.name,
        enabled: false,
        issues: preview.preview.issues,
        commands: preview.preview.commands,
        note: "Saved paused. Check the commands and prompts, then workflows.resume to enable it.",
      };
    }
    case "workflows.runs": {
      const current = find(await workflows.load(monoId));
      const limit = input.limit ?? RUNS_DEFAULT;
      if (!Number.isInteger(limit) || (limit as number) < 1 || (limit as number) > RUNS_MAX)
        throw new Error(`limit must be a whole number from 1 to ${RUNS_MAX}`);
      return {
        runs: (await workflows.runs(monoId, current.id))
          .slice(0, limit as number)
          .map(runDetail),
      };
    }
  }
  throw new Error(`Unknown app action: ${action}`);
}
