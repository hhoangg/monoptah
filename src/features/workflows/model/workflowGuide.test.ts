import { describe, expect, it } from "vitest";
import {
  DEFAULT_COMMAND_TIMEOUT_MINUTES,
  DEFAULT_MAX_PARALLEL,
  DEFAULT_SESSION_TIMEOUT_MINUTES,
  MAX_PARALLEL_LIMIT,
  MAX_ROUNDS_LIMIT,
  MAX_TIMEOUT_MINUTES,
  NODE_TYPES,
  WORKFLOWS_MAX,
  WORKFLOW_RUNS_KEPT,
  type Workflow,
} from "./workflow";
import { GUIDE_EXAMPLES, workflowGuide } from "./workflowGuide";
import { workflowIssues } from "./workflowModels";

/** The input values an example needs: a path for each project input, a word for the rest. */
function valuesFor(inputs: Workflow["inputs"]): Workflow["inputValues"] {
  return Object.fromEntries(
    Object.entries(inputs).map(([name, input]) => [
      name,
      input.type === "project" ? "/x" : input.type === "number" ? 1 : input.type === "boolean" ? true : "x",
    ]),
  );
}

describe("workflowGuide", () => {
  it("has at least the three required examples", () => {
    expect(GUIDE_EXAMPLES.length).toBeGreaterThanOrEqual(3);
    const names = GUIDE_EXAMPLES.map((example) => example.name).join(" | ");
    expect(names).toMatch(/linear/i);
    expect(names).toMatch(/fan-out|ticket/i);
    expect(names).toMatch(/\bIf\b/);
  });

  it("every guide example validates", () => {
    for (const example of GUIDE_EXAMPLES) {
      const issues = workflowIssues({
        ...example.workflow,
        id: "wf-aaaaaaaa",
        createdAt: 0,
        enabled: false,
        inputValues: valuesFor(example.workflow.inputs),
      });
      expect(issues, example.name).toEqual([]);
    }
  });

  it("guide embeds every example and the limits", () => {
    const guide = workflowGuide();
    for (const example of GUIDE_EXAMPLES) {
      expect(guide, example.name).toContain(JSON.stringify(example.workflow, null, 2));
      expect(guide).toContain(example.why);
    }
    for (const limit of [
      WORKFLOWS_MAX,
      WORKFLOW_RUNS_KEPT,
      MAX_PARALLEL_LIMIT,
      MAX_ROUNDS_LIMIT,
      MAX_TIMEOUT_MINUTES,
      DEFAULT_MAX_PARALLEL,
      DEFAULT_SESSION_TIMEOUT_MINUTES,
      DEFAULT_COMMAND_TIMEOUT_MINUTES,
    ])
      expect(guide).toContain(String(limit));
    expect(guide).toContain(`at most ${WORKFLOWS_MAX} workflows`);
    expect(guide).toContain("1440");
  });

  it("examples use catalog model ids", () => {
    const models = GUIDE_EXAMPLES.flatMap((example) =>
      example.workflow.nodes.flatMap((node) => (node.type === "session" && node.model ? [node.model] : [])),
    );
    expect(models.length).toBeGreaterThan(0);
    for (const model of models) expect(["claude:sonnet-5", "claude:opus-5"]).toContain(model);
  });

  it("guide mentions every node type", () => {
    const guide = workflowGuide();
    for (const type of NODE_TYPES) expect(guide).toContain(`\`${type}\``);
  });

  it("guide covers the rules the validator enforces", () => {
    const guide = workflowGuide();
    for (const topic of [
      "app models.list",
      "workflows.validate",
      "workflows.add",
      "workflows.update",
      "saved paused",
      "workflow-output",
      "inReport",
      "{{#if",
      "not empty",
      "loop",
      "maxRounds",
      "handle",
      "results",
      "Windows",
      "absolute path",
      "bare",
      "--",
      "eval",
      "backtick",
    ])
      expect(guide, topic).toContain(topic);
    // A loop condition is true to go back for another round, so an approval loop uses !=.
    expect(guide).toContain('{{review.verdict}} != "approve"');
  });

  it("guide has no dashes", () => {
    const guide = workflowGuide();
    expect(guide).not.toContain("—");
    expect(guide).not.toContain("–");
  });

  it("guide is deterministic", () => {
    expect(workflowGuide()).toBe(workflowGuide());
  });
});
