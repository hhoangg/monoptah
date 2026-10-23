import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  setHarnessAvailability,
  markHarnessAvailabilityProbed,
  type HarnessAvailability,
} from "../../../integrations/harness/core/availabilityState";
import type { Workflow, WorkflowNode } from "./workflow";
import { sessionModelIssues, workflowIssues } from "./workflowModels";
import { workflow } from "../ui/workflowUiFixtures";

const NONE: HarnessAvailability = {
  claude: false,
  codex: false,
  cursor: false,
  grok: false,
  opencode: false,
  pi: false,
  omp: false,
  fx: false,
  hermes: false,
  antigravity: false,
  devin: false,
};

const session = (extra: Partial<WorkflowNode> = {}): Workflow =>
  workflow({
    inputs: { project: { type: "project" } },
    inputValues: { project: "/work/api" },
    nodes: [
      { id: "trigger", type: "manual" },
      {
        id: "hi",
        type: "session",
        project: "{{inputs.project}}",
        prompt: "hi",
        checkout: "root",
        output: {},
        ...extra,
      } as WorkflowNode,
      { id: "report", type: "report" },
    ],
    edges: [
      { from: "trigger", to: "hi" },
      { from: "hi", to: "report" },
    ],
  });

const unavailable = (value: string) =>
  `Model "${value}" is not available. Pick one in the step's model list.`;

beforeEach(() => {
  setHarnessAvailability({ ...NONE, claude: true, grok: true });
  markHarnessAvailabilityProbed();
});
afterEach(() => setHarnessAvailability(NONE));

describe("sessionModelIssues", () => {
  it("flags the free-text model id a session was saved with", () => {
    expect(sessionModelIssues(session({ model: "claude-sonnet-5-5" }))).toEqual([
      { nodeId: "hi", message: unavailable("claude-sonnet-5-5") },
    ]);
  });

  it("flags a model name that is not an app id", () => {
    expect(sessionModelIssues(session({ model: "sonnet" }))).toEqual([
      { nodeId: "hi", message: unavailable("sonnet") },
    ]);
  });

  it("flags an unknown harness", () => {
    const issues = sessionModelIssues(session({ harness: "gpt" }));
    expect(issues).toHaveLength(1);
    expect(issues[0].nodeId).toBe("hi");
    expect(issues[0].message).toContain('"gpt"');
  });

  it("flags a harness that is not installed", () => {
    const issues = sessionModelIssues(session({ harness: "codex" }));
    expect(issues).toHaveLength(1);
    expect(issues[0].message).toContain('"codex"');
  });

  it("flags a model that belongs to another harness than the one set", () => {
    expect(
      sessionModelIssues(session({ harness: "grok", model: "claude:sonnet-5" })),
    ).toEqual([{ nodeId: "hi", message: unavailable("claude:sonnet-5") }]);
  });

  it("flags a model whose own harness is not installed", () => {
    expect(sessionModelIssues(session({ model: "cursor:grok-4.6" }))).toHaveLength(1);
  });

  it("flags an effort the model does not offer", () => {
    const issues = sessionModelIssues(
      session({ harness: "grok", model: "grok:grok-4.6", effort: "turbo" }),
    );
    expect(issues).toHaveLength(1);
    expect(issues[0].message).toContain('"turbo"');
  });

  it("flags an effort on a model with no effort setting", () => {
    expect(
      sessionModelIssues(
        session({ harness: "claude", model: "claude:sonnet-4.6", effort: "high" }),
      ),
    ).toHaveLength(1);
  });

  it("accepts exact ids and a valid effort", () => {
    expect(
      sessionModelIssues(
        session({ harness: "grok", model: "grok:grok-4.6", effort: "xhigh" }),
      ),
    ).toEqual([]);
    expect(sessionModelIssues(session({ model: "claude:sonnet-5" }))).toEqual([]);
    expect(sessionModelIssues(session({ harness: "claude" }))).toEqual([]);
  });

  it("raises nothing for a session that sets nothing", () => {
    expect(sessionModelIssues(session())).toEqual([]);
  });

  it("does not judge availability before the first probe", async () => {
    // The probe flag cannot be reset, so load a fresh copy of the modules.
    vi.resetModules();
    const fresh = await import("./workflowModels");
    expect(
      fresh.sessionModelIssues(session({ harness: "cursor", model: "cursor:grok-4.6" })),
    ).toEqual([]);
  });

  it("ignores nodes that are not sessions", () => {
    expect(sessionModelIssues(workflow())).toEqual([]);
  });
});

describe("workflowIssues", () => {
  it("adds the model issues to the graph issues", () => {
    const w = session({ model: "sonnet" });
    expect(workflowIssues(w)).toEqual([
      { nodeId: "hi", message: unavailable("sonnet") },
    ]);
    expect(workflowIssues({ ...w, nodes: [] }).length).toBeGreaterThan(0);
  });
});
