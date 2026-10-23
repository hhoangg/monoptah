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
  WORKFLOW_DEFINITION_MAX_BYTES,
  WORKFLOW_DESCRIPTION_MAX,
  WORKFLOW_NAME_MAX,
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
    expect(guide).toContain(`at most ${WORKFLOWS_MAX} workflows`);
    expect(guide).toContain(`last ${WORKFLOW_RUNS_KEPT} runs`);
    expect(guide).toContain(`1 to ${MAX_PARALLEL_LIMIT} (default ${DEFAULT_MAX_PARALLEL})`);
    expect(guide).toContain(`\`maxRounds\` 1 to ${MAX_ROUNDS_LIMIT}`);
    expect(guide).toContain(`at most ${MAX_TIMEOUT_MINUTES} (24 hours)`);
    expect(guide).toContain(`defaults ${DEFAULT_SESSION_TIMEOUT_MINUTES} for a session and ${DEFAULT_COMMAND_TIMEOUT_MINUTES} for a command`);
    expect(guide).toContain(`under ${WORKFLOW_DEFINITION_MAX_BYTES / 1024} KiB`);
    expect(guide).toContain(`at most ${WORKFLOW_NAME_MAX} characters`);
    expect(guide).toContain(`at most ${WORKFLOW_DESCRIPTION_MAX} characters`);
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

  it("guide states the points reviewers asked for", () => {
    const guide = workflowGuide();
    expect(guide).toContain("only your template");
    expect(guide).toContain("only if the template prints `{{results}}`");
    expect(guide).toContain("upstream of the report");
    expect(guide).toContain("`Run` entry");
    expect(guide).toContain("problems with the graph as a whole");
    expect(guide).toContain("pass its `id`");
    expect(guide).toContain("does not count against it");
    expect(guide).toContain("`name`, `description`, `inputs`, `inputValues`, `nodes` and `edges`");
    expect(guide).toContain("checks every condition against this grammar");
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
    expect(guide).not.toContain("\u2014");
    expect(guide).not.toContain("\u2013");
  });

  it("guide is deterministic", () => {
    expect(workflowGuide()).toBe(workflowGuide());
  });
});
