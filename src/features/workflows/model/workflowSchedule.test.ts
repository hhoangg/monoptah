import { describe, expect, it } from "vitest";
import type { Workflow } from "./workflow";
import { afterClaim, dueWorkflows } from "./workflowSchedule";

const NOW = new Date(2026, 9, 22, 12).getTime();

function workflow(extra: Partial<Workflow> = {}): Workflow {
  return {
    id: "wf-abcd1234",
    name: "Ship it",
    inputs: {},
    inputValues: {},
    nodes: [
      {
        id: "trigger",
        type: "schedule",
        schedule: {
          scheduleKind: "daily",
          time: "09:00",
          minute: 0,
          dayOfWeek: 1,
        },
      },
    ],
    edges: [],
    enabled: true,
    createdAt: 0,
    nextRunAt: NOW - 1_000,
    ...extra,
  };
}

describe("afterClaim", () => {
  it("a claim clears the run request and any stop asked for before it", () => {
    const claimed = afterClaim(
      workflow({ runRequested: true, stopRequested: true }),
      NOW,
      "run-1",
    );
    expect(claimed.runRequested).toBeUndefined();
    expect(claimed.stopRequested).toBeUndefined();
    expect(claimed.lastRunId).toBe("run-1");
    expect(claimed.nextRunAt).toBeGreaterThan(NOW);
  });

  it("moving the schedule on without a claim keeps a stop written for the run that is going", () => {
    const advanced = afterClaim(workflow({ stopRequested: true }), NOW);
    expect(advanced.stopRequested).toBe(true);
    expect(advanced.nextRunAt).toBeGreaterThan(NOW);
  });
});

describe("dueWorkflows", () => {
  it("skips and advances a due workflow whose run is still going", () => {
    const { run, advance } = dueWorkflows([workflow()], NOW, () => true);
    expect(run).toEqual([]);
    expect(advance).toHaveLength(1);
  });
});
