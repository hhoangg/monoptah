import {
  MonoFileConflict,
  readAgentFile,
  writeAgentFile,
} from "../../monos/model/monoFiles";
import {
  WORKFLOW_RUNS_KEPT,
  type NodeRun,
  type NodeStatus,
  type RoundRecord,
  type Workflow,
  type WorkflowId,
  type WorkflowRun,
} from "./workflow";
import { readWorkflowFile, type WorkflowFile } from "./workflowParse";

/** The Mono's workflows from its folder. */
export async function loadWorkflows(monoId: string): Promise<Workflow[]> {
  return (
    readWorkflowFile((await readAgentFile(monoId, "workflows.json")).text ?? "")
      ?.workflows ?? []
  );
}

/** Stay under the 2 MiB cap `mono.rs` puts on workflows.json. */
export const WORKFLOWS_FILE_BUDGET = 2 * 1024 * 1024;

function serializeWorkflows(workflows: Workflow[], unreadable: unknown[]): string {
  const text = `${JSON.stringify([...workflows, ...unreadable], null, 2)}\n`;
  // Say so here rather than let the host fail with a raw "is too large".
  if (new TextEncoder().encode(text).length > WORKFLOWS_FILE_BUDGET)
    throw new Error(
      "The workflows are too large to save (over 2 MiB in total). Remove a workflow or shorten its steps.",
    );
  return text;
}

/** The file can only be changed through this app's own format; anything else is left for the user to fix. */
function unreadable(file: string, what: string): Error {
  return new Error(
    `${file} could not be read; fix or remove the file before changing ${what}`,
  );
}

const idOf = (raw: unknown): string | undefined =>
  raw && typeof raw === "object" && typeof (raw as { id?: unknown }).id === "string"
    ? (raw as { id: string }).id
    : undefined;

/**
 * Change workflows.json over the version that was read, retrying when the
 * panel or another run saved in between. Entries this app cannot read (made
 * by a newer version, or edited by hand) are written back untouched after the
 * others, and a file that is not a JSON list is never overwritten.
 */
export async function updateWorkflows<T>(
  monoId: string,
  edit: (list: Workflow[]) => { workflows: Workflow[]; result: T },
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const file = await readAgentFile(monoId, "workflows.json");
    const current: WorkflowFile | undefined = readWorkflowFile(file.text ?? "");
    if (!current) throw unreadable("workflows.json", "workflows");
    const { workflows, result } = edit(current.workflows);
    // An id the file already uses, in an entry that cannot be read, must not be used again.
    const known = new Set(current.workflows.map((w) => w.id));
    const taken = new Set(current.unreadable.map(idOf));
    for (const w of workflows) {
      if (!known.has(w.id) && taken.has(w.id))
        throw new Error(
          `workflows.json already has an entry with the id "${w.id}" that could not be read; fix or remove it first`,
        );
    }
    // A leftover that repeats the id of a workflow the edit removed would come back as that
    // workflow on the next read, so it goes with it.
    const kept = new Set(workflows.map((w) => w.id));
    const leftovers = current.unreadable.filter((raw) => {
      const id = idOf(raw);
      return id === undefined || !known.has(id) || kept.has(id);
    });
    try {
      await writeAgentFile(
        monoId,
        "workflows.json",
        serializeWorkflows(workflows, leftovers),
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

/**
 * Runs are written by the app, so only the shape the engine relies on is checked. Undefined when
 * the text is not a JSON list, so a caller can refuse to overwrite it.
 */
function readRunsFile(
  text: string | null | undefined,
): { runs: WorkflowRun[]; unreadable: unknown[] } | undefined {
  if (!text?.trim()) return { runs: [], unreadable: [] };
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return undefined;
  }
  if (!Array.isArray(parsed)) return undefined;
  const runs: WorkflowRun[] = [];
  const unreadable: unknown[] = [];
  for (const raw of parsed) {
    if (isRun(raw)) runs.push(withCleanHistory(raw));
    else unreadable.push(raw);
  }
  return { runs, unreadable };
}

const NODE_STATUSES: ReadonlySet<string> = new Set<NodeStatus>([
  "pending",
  "running",
  "waitingApproval",
  "done",
  "failed",
  "skipped",
  "stopped",
]);

const isPlainObject = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === "object" && !Array.isArray(v);
const isOptionalNumber = (v: unknown) => v === undefined || typeof v === "number";
const isOptionalString = (v: unknown) => v === undefined || typeof v === "string";

function isRoundRecord(raw: unknown): raw is RoundRecord {
  if (!isPlainObject(raw)) return false;
  const { decision } = raw;
  return (
    typeof raw.round === "number" &&
    typeof raw.status === "string" &&
    NODE_STATUSES.has(raw.status) &&
    isOptionalString(raw.sessionId) &&
    isOptionalString(raw.error) &&
    isOptionalNumber(raw.startedAt) &&
    isOptionalNumber(raw.endedAt) &&
    (decision === undefined ||
      (isPlainObject(decision) &&
        typeof decision.value === "string" &&
        isOptionalString(decision.reason)))
  );
}

/** Drops the per-round records of a run that cannot be read; the rest of the run stays as saved. */
function withCleanHistory(run: WorkflowRun): WorkflowRun {
  if (!run.nodes.some((n) => isPlainObject(n) && "history" in n)) return run;
  return {
    ...run,
    nodes: run.nodes.map((n) => {
      if (!isPlainObject(n) || !("history" in n)) return n;
      const { history, ...rest } = n as NodeRun;
      // Records saved before rounds stopped keeping their output still carry it; it is dropped.
      const kept = Array.isArray(history)
        ? history.filter(isRoundRecord).map((record) => {
            const { output: _output, ...rest } = record as RoundRecord & { output?: unknown };
            return rest;
          })
        : [];
      return kept.length > 0 ? { ...rest, history: kept } : rest;
    }),
  };
}

function isRun(raw: unknown): raw is WorkflowRun {
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
}

/** A workflow's runs, newest first. A workflow that never ran has none. */
export async function loadRuns(
  monoId: string,
  workflowId: WorkflowId,
): Promise<WorkflowRun[]> {
  return (
    readRunsFile((await readAgentFile(monoId, runsPath(workflowId))).text)
      ?.runs ?? []
  );
}

/** Stay under the 4 MiB cap `mono.rs` puts on a runs file. */
export const RUNS_FILE_BUDGET = 3.5 * 1024 * 1024;

/**
 * The newest runs that fit: at most 20 and under the size budget, never fewer than one. Entries
 * that could not be read follow them, untouched.
 */
function serializeRuns(runs: WorkflowRun[], unreadable: unknown[]): string {
  let kept = runs.slice(0, WORKFLOW_RUNS_KEPT);
  const write = () => `${JSON.stringify([...kept, ...unreadable], null, 2)}\n`;
  let text = write();
  while (
    kept.length > 1 &&
    new TextEncoder().encode(text).length > RUNS_FILE_BUDGET
  ) {
    kept = kept.slice(0, -1);
    text = write();
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
    const current = readRunsFile(file.text);
    if (!current) throw unreadable(runsPath(workflowId), "the run history");
    const { runs, result } = edit(current.runs);
    try {
      await writeAgentFile(
        monoId,
        runsPath(workflowId),
        serializeRuns(runs, current.unreadable),
        file.hash,
      );
      return result;
    } catch (error) {
      if (!(error instanceof MonoFileConflict) || attempt >= 2) throw error;
    }
  }
}

/**
 * Empties the run history of a workflow that was removed, so nothing of it stays on disk.
 * Failing to is harmless, so it never rejects.
 */
export async function clearWorkflowRuns(
  monoId: string,
  workflowId: WorkflowId,
): Promise<void> {
  try {
    await writeAgentFile(monoId, runsPath(workflowId), "[]\n");
  } catch (error) {
    console.warn("Could not clear the runs of a removed workflow", error);
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

export type ResumeRequest = {
  monoId: string;
  workflowId: WorkflowId;
  runId: string;
};

/**
 * Takes the requests other windows left in the workflow files, clearing them in the same
 * hash-checked save, so of several drivers looking at once exactly one gets each resume.
 * A stop request is cleared only when `isRunning` says no window has a run of that workflow
 * (it is stale); one for a run that is going stays for the window driving it to apply.
 */
export function takeWorkflowRequests(
  monoId: string,
  isRunning: (workflowId: WorkflowId) => boolean,
): Promise<ResumeRequest[]> {
  return updateWorkflows(monoId, (list) => {
    const resumes: ResumeRequest[] = [];
    const workflows = list.map((w) => {
      const { stopRequested, resumeRunId, ...rest } = w;
      if (resumeRunId)
        resumes.push({ monoId, workflowId: w.id, runId: resumeRunId });
      // A stop from before a resume was meant for the run that was interrupted.
      const keepStop = stopRequested && isRunning(w.id);
      return { ...rest, ...(keepStop ? { stopRequested } : {}) };
    });
    return { workflows, result: resumes };
  });
}

/** A copy of the runs in flight in this window, by workflow. */
export function liveWorkflowRuns(): ReadonlyMap<WorkflowId, string> {
  return new Map(running);
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
 * Stops a workflow's running run from any window. Only one window drives runs (see
 * `workflowDriver`), so the request is also written to the workflow's file, where the driver
 * picks it up within seconds. Here it takes effect at once when this window is the driver.
 */
export async function stopWorkflowRun(
  monoId: string,
  workflowId: WorkflowId,
): Promise<void> {
  // Runs live in the driver's memory only: a flag set anywhere else would never be cleared.
  if (workflowRunningId(workflowId)) requestWorkflowStop(workflowId);
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
