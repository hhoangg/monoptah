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

/** The ClickUp -> Dev -> Review example from the design spec (section 9.1), laid out on the board. */
export function ticketWorkflow(extra: Partial<Workflow> = {}): Workflow {
  return {
    id: "wf-ticket001",
    name: "ClickUp -> Dev -> Review",
    description: "Pull my ClickUp tickets, implement each, review up to 3 rounds.",
    inputs: {
      project: { type: "project" },
      clickupList: { type: "string", description: "ClickUp list to pull from" },
    },
    inputValues: { project: "/work/api", clickupList: "Sprint" },
    nodes: [
      { id: "trigger", type: "schedule", schedule: SCHEDULE, title: "Weekdays", ui: { x: 0, y: 0 } },
      {
        id: "fetch",
        type: "session",
        title: "Fetch tickets",
        project: "{{inputs.project}}",
        model: "sonnet",
        checkout: "root",
        prompt: "List open tickets assigned to me in {{inputs.clickupList}}.",
        output: { tickets: { type: "list" } },
        ui: { x: 0, y: 90 },
      },
      {
        id: "each",
        type: "forEach",
        title: "Each ticket",
        in: "{{fetch.tickets}}",
        as: "item",
        maxParallel: 3,
        ui: { x: -16, y: 190 },
      },
      {
        id: "dev",
        type: "session",
        title: "Dev",
        parent: "each",
        project: "{{inputs.project}}",
        model: "sonnet",
        checkout: "newWorktree",
        prompt:
          "Implement {{item.title}} ({{item.url}}) and open a PR.\n" +
          "{{#if review}}Fix these review issues: {{review.issues}}{{/if}}",
        output: { pr_url: { type: "string" }, summary: { type: "string" } },
        ui: { x: 16, y: 40 },
      },
      {
        id: "review",
        type: "session",
        title: "Review",
        parent: "each",
        project: "{{inputs.project}}",
        model: "opus",
        checkout: { worktreeOf: "dev" },
        nestUnder: "dev",
        prompt: "Review {{dev.pr_url}}. List blocking issues only.",
        output: { verdict: { type: "string" }, issues: { type: "list" } },
        ui: { x: 16, y: 130 },
      },
      { id: "report", type: "report", title: "Report to Mono", ui: { x: 0, y: 480 } },
    ],
    edges: [
      { from: "trigger", to: "fetch" },
      { from: "fetch", to: "each" },
      { from: "dev", to: "review" },
      { from: "each", to: "report" },
      { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
    ],
    enabled: true,
    createdAt: 1,
    ...extra,
  };
}
