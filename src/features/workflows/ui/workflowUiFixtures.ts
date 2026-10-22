import type { Workflow, WorkflowNode, WorkflowRun } from "../model/workflow";

/** Shared builders for the workflow panel tests. */
export const SCHEDULE = {
  scheduleKind: "weekdays",
  time: "09:00",
  minute: 0,
  dayOfWeek: 1,
} as const;

export const MANUAL: WorkflowNode = { id: "trigger", type: "manual" };
export const REPORT: WorkflowNode = { id: "report", type: "report" };

export function workflow(extra: Partial<Workflow> = {}): Workflow {
  return {
    id: "wf-aaaaaaaa",
    name: "Nightly checks",
    inputs: {},
    inputValues: {},
    nodes: [MANUAL, REPORT],
    edges: [{ from: "trigger", to: "report" }],
    enabled: true,
    createdAt: 1,
    ...extra,
  };
}

export function scheduled(extra: Partial<Workflow> = {}): Workflow {
  return workflow({
    nodes: [{ id: "trigger", type: "schedule", schedule: SCHEDULE }, REPORT],
    nextRunAt: Date.now() + 3_600_000,
    ...extra,
  });
}

export function run(extra: Partial<WorkflowRun> = {}): WorkflowRun {
  return {
    id: "run-1",
    workflowId: "wf-aaaaaaaa",
    definition: workflow(),
    status: "done",
    startedAt: Date.now() - 120_000,
    endedAt: Date.now() - 60_000,
    trigger: "manual",
    items: {},
    nodes: [],
    ...extra,
  };
}
