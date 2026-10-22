import {
  MonoFileConflict,
  readAgentFile,
  writeAgentFile,
} from "../../monos/model/monoFiles";
import {
  WORKFLOW_RUNS_KEPT,
  type Workflow,
  type WorkflowId,
  type WorkflowRun,
} from "./workflow";
import { parseWorkflows } from "./workflowParse";

/** The Mono's workflows from its folder. */
export async function loadWorkflows(monoId: string): Promise<Workflow[]> {
  return parseWorkflows(
    (await readAgentFile(monoId, "workflows.json")).text ?? "",
  );
}

function serializeWorkflows(workflows: Workflow[]): string {
  return `${JSON.stringify(workflows, null, 2)}\n`;
}

/**
 * Change workflows.json over the version that was read, retrying when the
 * panel or another run saved in between.
 */
export async function updateWorkflows<T>(
  monoId: string,
  edit: (list: Workflow[]) => { workflows: Workflow[]; result: T },
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const file = await readAgentFile(monoId, "workflows.json");
    const { workflows, result } = edit(parseWorkflows(file.text ?? ""));
    try {
      await writeAgentFile(
        monoId,
        "workflows.json",
        serializeWorkflows(workflows),
        file.hash,
      );
      return result;
    } catch (error) {
      if (!(error instanceof MonoFileConflict) || attempt >= 2) throw error;
    }
  }
}

function runsPath(workflowId: WorkflowId) {
  return `workflow-runs/${workflowId}.json` as const;
}

/** Runs are written by the app, so only the shape the engine relies on is checked. */
function parseRuns(text: string | null | undefined): WorkflowRun[] {
  if (!text?.trim()) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.filter((raw): raw is WorkflowRun => {
    if (!raw || typeof raw !== "object") return false;
    const run = raw as Record<string, unknown>;
    return (
      typeof run.id === "string" &&
      typeof run.workflowId === "string" &&
      typeof run.status === "string" &&
      typeof run.startedAt === "number" &&
      !!run.definition &&
      typeof run.definition === "object" &&
      Array.isArray(run.nodes)
    );
  });
}

/** A workflow's runs, newest first. A workflow that never ran has none. */
export async function loadRuns(
  monoId: string,
  workflowId: WorkflowId,
): Promise<WorkflowRun[]> {
  return parseRuns((await readAgentFile(monoId, runsPath(workflowId))).text);
}

/** Stay under the 4 MiB cap `mono.rs` puts on a runs file. */
export const RUNS_FILE_BUDGET = 3.5 * 1024 * 1024;

/** The newest runs that fit: at most 20 and under the size budget, never fewer than one. */
function serializeRuns(runs: WorkflowRun[]): string {
  let kept = runs.slice(0, WORKFLOW_RUNS_KEPT);
  let text = `${JSON.stringify(kept, null, 2)}\n`;
  while (
    kept.length > 1 &&
    new TextEncoder().encode(text).length > RUNS_FILE_BUDGET
  ) {
    kept = kept.slice(0, -1);
    text = `${JSON.stringify(kept, null, 2)}\n`;
  }
  return text;
}

/** Change a workflow's run history the same way, keeping only the newest runs. */
export async function updateRuns<T>(
  monoId: string,
  workflowId: WorkflowId,
  edit: (runs: WorkflowRun[]) => { runs: WorkflowRun[]; result: T },
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const file = await readAgentFile(monoId, runsPath(workflowId));
    const { runs, result } = edit(parseRuns(file.text));
    try {
      await writeAgentFile(
        monoId,
        runsPath(workflowId),
        serializeRuns(runs),
        file.hash,
      );
      return result;
    } catch (error) {
      if (!(error instanceof MonoFileConflict) || attempt >= 2) throw error;
    }
  }
}

const ACTIVE_CHANGED = "monocode:mono-workflows-active";

/** Workflow runs in flight, to their run id. In memory only, like the run itself. */
const running = new Map<WorkflowId, string>();
const stops = new Set<WorkflowId>();

/** Marks a workflow as running (or, with null, no longer running). */
export function setWorkflowRunning(
  workflowId: WorkflowId,
  runId: string | null,
): void {
  if (runId == null) running.delete(workflowId);
  else running.set(workflowId, runId);
  window.dispatchEvent(new CustomEvent(ACTIVE_CHANGED));
}

export function workflowRunningId(workflowId: WorkflowId): string | undefined {
  return running.get(workflowId);
}

export function subscribeWorkflowsRunning(listener: () => void): () => void {
  window.addEventListener(ACTIVE_CHANGED, listener);
  return () => window.removeEventListener(ACTIVE_CHANGED, listener);
}

export function requestWorkflowStop(workflowId: WorkflowId): void {
  stops.add(workflowId);
  window.dispatchEvent(new CustomEvent(ACTIVE_CHANGED));
}

/**
 * Stops a workflow's running run from any window. Only the main window drives runs, so the
 * request is also written to the workflow's file, where the driver picks it up within seconds.
 */
export async function stopWorkflowRun(
  monoId: string,
  workflowId: WorkflowId,
): Promise<void> {
  requestWorkflowStop(workflowId);
  await updateWorkflows(monoId, (list) => ({
    workflows: list.map((w) =>
      w.id === workflowId ? { ...w, stopRequested: true } : w,
    ),
    result: undefined,
  }));
}

export function workflowStopRequested(workflowId: WorkflowId): boolean {
  return stops.has(workflowId);
}

export function clearWorkflowStop(workflowId: WorkflowId): void {
  if (stops.delete(workflowId))
    window.dispatchEvent(new CustomEvent(ACTIVE_CHANGED));
}

const CHECK_NOW = "monocode:mono-workflows-check-now";

/** Ask the scheduler to look for due workflows now rather than at its next minute. */
export function checkWorkflowsNow(): void {
  window.dispatchEvent(new CustomEvent(CHECK_NOW));
}

export function subscribeCheckWorkflowsNow(listener: () => void): () => void {
  window.addEventListener(CHECK_NOW, listener);
  return () => window.removeEventListener(CHECK_NOW, listener);
}
