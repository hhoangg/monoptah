import { describe, expect, it } from "vitest";
import type { NodeRun, RunStatus, Workflow, WorkflowEdge, WorkflowNode, WorkflowRun } from "./workflow";
import type { EngineAction } from "./workflowEngine";
import { askMonoPrompt, composeReport, fieldInReport, parseMonoDecision } from "./workflowReport";

const PROJECT = "{{inputs.project}}";

function def(nodes: WorkflowNode[], edges: WorkflowEdge[]): Workflow {
  return {
    id: "wf-aaaaaaaa",
    name: "Ship tickets",
    inputs: { project: { type: "project" } },
    inputValues: { project: "/work/repo" },
    nodes,
    edges,
    enabled: true,
    createdAt: 1,
  };
}

const trigger: WorkflowNode = { id: "trigger", type: "manual" };
const report: WorkflowNode = { id: "report", type: "report" };

function session(
  id: string,
  extra: Partial<Extract<WorkflowNode, { type: "session" }>> = {},
): WorkflowNode {
  return { id, type: "session", project: PROJECT, checkout: "root", prompt: `Do ${id}`, output: {}, ...extra };
}

function nodeRun(partial: Partial<NodeRun> & Pick<NodeRun, "nodeId">): NodeRun {
  return { branch: "", status: "done", round: 1, ...partial };
}

function runOf(
  definition: Workflow,
  nodes: NodeRun[],
  extra: Partial<WorkflowRun> = {},
): WorkflowRun {
  return {
    id: "run-1",
    workflowId: definition.id,
    definition,
    status: "done",
    startedAt: 1_000,
    endedAt: 2_000,
    trigger: "manual",
    items: {},
    nodes,
    ...extra,
  };
}

/** trigger -> each (forEach) -> dev (session with a PR link) -> report. */
function fanOut(devExtra: Partial<Extract<WorkflowNode, { type: "session" }>> = {}) {
  return def(
    [
      trigger,
      { id: "each", type: "forEach", in: "{{items}}" },
      session("dev", {
        parent: "each",
        output: { pr_url: { type: "string" }, files: { type: "list" } },
        ...devExtra,
      }),
      report,
    ],
    [
      { from: "trigger", to: "each" },
      { from: "each", to: "report" },
    ],
  );
}

describe("composeReport", () => {
  it("report lists each branch with PR link", () => {
    const d = fanOut();
    const run = runOf(
      d,
      [
        nodeRun({ nodeId: "dev", branch: "each#0", output: { pr_url: "https://github.com/o/r/pull/1", files: ["a.ts"] }, endedAt: 5 }),
        nodeRun({ nodeId: "dev", branch: "each#1", status: "failed", error: "tests did not pass" }),
        nodeRun({ nodeId: "dev", branch: "each#2", output: { pr_url: "https://github.com/o/r/pull/3" }, endedAt: 7 }),
      ],
      {
        status: "doneWithFailures",
        items: { each: [{ title: "Fix login" }, { name: "Add dark mode" }, "plain"] },
      },
    );
    const { title, text } = composeReport(run);
    expect(title).toBe("Ship tickets · completed with failures");
    expect(text).toContain("Fix login");
    expect(text).toContain("https://github.com/o/r/pull/1");
    expect(text).toContain("Add dark mode");
    expect(text).toContain("tests did not pass");
    expect(text).toContain("#3");
    expect(text).toContain("https://github.com/o/r/pull/3");
    // The list field is off by default, so it is not rendered.
    expect(text).not.toContain("a.ts");
    expect(text).toContain("failed");
  });

  it("lists a failed branch of an earlier loop round and keeps the latest round as the main line", () => {
    const d = fanOut();
    const run = runOf(
      d,
      [
        nodeRun({ nodeId: "dev", branch: "each#0", output: { pr_url: "https://x/new" }, endedAt: 9 }),
        nodeRun({ nodeId: "dev", branch: "each#0@r1", status: "failed", error: "tests did not pass" }),
        nodeRun({ nodeId: "dev", branch: "each#1@r1", output: { pr_url: "https://x/old-ok" }, endedAt: 4 }),
        nodeRun({ nodeId: "dev", branch: "each#1", output: { pr_url: "https://x/ok" }, endedAt: 9 }),
      ],
      {
        status: "doneWithFailures",
        items: { each: [{ title: "Fix login" }, { title: "Billing" }], "each@r1": [{ title: "Fix login" }, { title: "Billing" }] },
      },
    );
    const { text } = composeReport(run);
    expect(text).toContain("### Fix login\n- Status: done");
    expect(text).toContain("https://x/new");
    expect(text).toContain("### Fix login · round 1\n- Status: failed");
    expect(text).toContain("Error in dev: tests did not pass");
    // A round that passed is not repeated.
    expect(text).not.toContain("Billing · round 1");
    expect(text).not.toContain("https://x/old-ok");
  });

  it("lists an archived branch that hit max rounds", () => {
    const d = fanOut();
    const run = runOf(
      d,
      [
        nodeRun({ nodeId: "dev", branch: "each#0", output: { pr_url: "https://x/1" }, endedAt: 9 }),
        nodeRun({ nodeId: "dev", branch: "each#0@r1", decision: { value: "exit", reason: "max rounds reached" } }),
      ],
      { items: { each: [{ title: "A" }], "each@r1": [{ title: "A" }] } },
    );
    expect(composeReport(run).text).toContain("### A · round 1");
  });

  it("shows top-level session outputs next to a For each", () => {
    const d = def(
      [
        trigger,
        { id: "each", type: "forEach", in: "{{items}}" },
        session("dev", { parent: "each", output: { pr_url: { type: "string" } } }),
        session("summary", { output: { digest: { type: "string" }, raw: { type: "list" } } }),
        report,
      ],
      [
        { from: "trigger", to: "each" },
        { from: "each", to: "summary" },
        { from: "summary", to: "report" },
      ],
    );
    const run = runOf(
      d,
      [
        nodeRun({ nodeId: "dev", branch: "each#0", output: { pr_url: "https://x/1" }, endedAt: 2 }),
        nodeRun({ nodeId: "summary", output: { digest: "One PR opened", raw: ["x"] }, endedAt: 3 }),
      ],
      { items: { each: [{ title: "T" }] } },
    );
    const { text } = composeReport(run);
    expect(text).toContain("summary.digest: One PR opened");
    expect(text).not.toContain("raw");
    expect(text).toContain("https://x/1");
    // Without any reported top-level output there is no extra section.
    const quiet = composeReport(runOf(d, [run.nodes[0]], { items: run.items })).text;
    expect(quiet).not.toContain("### Run");
  });

  it("report flags max rounds", () => {
    const d = def(
      [
        trigger,
        session("dev", { output: { pr_url: { type: "string" } } }),
        session("review", { output: { verdict: { type: "string" } } }),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "review" },
        { from: "review", to: "report" },
        { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
      ],
    );
    const run = runOf(d, [
      nodeRun({ nodeId: "dev", round: 3, output: { pr_url: "https://x/1" }, endedAt: 3 }),
      nodeRun({
        nodeId: "review",
        round: 3,
        output: { verdict: "changes" },
        decision: { value: "exit", reason: "max rounds reached" },
        endedAt: 4,
      }),
    ]);
    const { text } = composeReport(run);
    expect(text).toContain("Max rounds reached");
    expect(text).toMatch(/Loop rounds used: 3/);
    // The last session node is review, so its verdict is shown.
    expect(text).toContain("changes");
  });

  it("shows when a loop ended because the branch failed", () => {
    const d = def(
      [trigger, session("dev"), session("review"), report],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "review" },
        { from: "review", to: "report" },
        { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
      ],
    );
    const run = runOf(
      d,
      [
        nodeRun({ nodeId: "dev", status: "failed", error: "boom", round: 2 }),
        nodeRun({ nodeId: "review", decision: { value: "exit", reason: "branch failed" }, round: 1 }),
      ],
      { status: "failed" },
    );
    const { title, text } = composeReport(run);
    expect(title).toBe("Ship tickets · failed");
    expect(text).toContain("branch failed");
    expect(text).toContain("boom");
    expect(text).not.toContain("Max rounds reached");
  });

  it("report for empty forEach says No items", () => {
    const d = fanOut();
    const run = runOf(d, [], { items: { each: [] } });
    const { text } = composeReport(run);
    expect(text).toContain("No items");
  });

  it("template override renders", () => {
    const d = def(
      [
        trigger,
        { id: "each", type: "forEach", in: "{{items}}" },
        session("dev", { parent: "each", output: { pr_url: { type: "string" } } }),
        { id: "report", type: "report", template: "RUN {{run.id}} is {{run.status}}\n{{results}}" },
      ],
      [
        { from: "trigger", to: "each" },
        { from: "each", to: "report" },
      ],
    );
    const run = runOf(
      d,
      [nodeRun({ nodeId: "dev", branch: "each#0", output: { pr_url: "https://x/9" }, endedAt: 2 })],
      { items: { each: [{ title: "T" }] } },
    );
    const { title, text } = composeReport(run);
    expect(title).toBe("Ship tickets · completed");
    expect(text).toContain("RUN run-1 is done");
    expect(text).toContain("https://x/9");
    // The default per-branch layout is replaced.
    expect(text).not.toContain("Loop rounds");
    expect(text).not.toContain("### ");
  });

  it("honours inReport overrides", () => {
    const d = def(
      [
        trigger,
        session("dev", {
          output: {
            summary: { type: "string", inReport: false },
            files: { type: "list", inReport: true },
            n: { type: "number" },
            ok: { type: "boolean", inReport: true },
          },
        }),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "report" },
      ],
    );
    const run = runOf(d, [
      nodeRun({
        nodeId: "dev",
        output: { summary: "hidden text", files: ["a.ts", "b.ts"], n: 42, ok: true },
        endedAt: 1,
      }),
    ]);
    const { text } = composeReport(run);
    expect(text).not.toContain("hidden text");
    expect(text).toContain("a.ts");
    expect(text).toContain("b.ts");
    expect(text).not.toContain("42");
    expect(text).toContain("true");
  });

  it("keeps short string values on one line", () => {
    const d = def([trigger, session("dev", { output: { note: { type: "string" } } }), report], [
      { from: "trigger", to: "dev" },
      { from: "dev", to: "report" },
    ]);
    const run = runOf(d, [nodeRun({ nodeId: "dev", output: { note: "line one\nline two" }, endedAt: 1 })]);
    const { text } = composeReport(run);
    expect(text).toContain("note: line one line two");
  });

  it.each<[RunStatus, string]>([
    ["done", "completed"],
    ["doneWithFailures", "completed with failures"],
    ["failed", "failed"],
    ["stopped", "stopped"],
    ["interrupted", "interrupted"],
  ])("labels %s as %s", (status, label) => {
    const run = runOf(def([trigger, report], [{ from: "trigger", to: "report" }]), [], { status });
    expect(composeReport(run).title).toBe(`Ship tickets · ${label}`);
  });
});

describe("askMonoPrompt", () => {
  const action: Extract<EngineAction, { kind: "askMono" }> = {
    kind: "askMono",
    nodeId: "gate",
    branch: "each#0",
    purpose: "askMono",
    question: "Is the PR safe to merge?",
    answers: ["yes", "no"],
    context: { dev: { pr_url: "https://x/1" }, round: 2 },
  };

  it("tells the Mono it is consulted by a workflow and asks for a workflow-output block", () => {
    const text = askMonoPrompt(action, "Ship tickets");
    expect(text).toContain("Ship tickets");
    expect(text).toContain("running workflow");
    expect(text).toContain("Is the PR safe to merge?");
    expect(text).toContain(JSON.stringify(action.context, null, 2));
    expect(text).toContain('"yes"');
    expect(text).toContain('"no"');
    expect(text.trimEnd().endsWith("```")).toBe(true);
    expect(text).toContain("```workflow-output");
    expect(text).toContain('{"answer":');
  });
});

describe("parseMonoDecision", () => {
  const reply = (body: string) => `Thinking out loud.\n\n\`\`\`workflow-output\n${body}\n\`\`\``;

  it("accepts an allowed answer with a reason", () => {
    expect(parseMonoDecision(reply('{"answer":"yes","reason":"all green"}'), ["yes", "no"])).toEqual({
      ok: true,
      value: "yes",
      reason: "all green",
    });
  });

  it("treats the reason as optional", () => {
    expect(parseMonoDecision(reply('{"answer":"no"}'), ["yes", "no"])).toEqual({ ok: true, value: "no" });
  });

  it("parseMonoDecision rejects answers outside the list", () => {
    const result = parseMonoDecision(reply('{"answer":"maybe"}'), ["yes", "no"]);
    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error).toContain("maybe");
      expect(result.error).toContain("yes");
    }
  });

  it("rejects a reply without the block and a non-string answer", () => {
    expect(parseMonoDecision("yes", ["yes"]).ok).toBe(false);
    expect(parseMonoDecision(reply('{"answer":1}'), ["1"]).ok).toBe(false);
    expect(parseMonoDecision(reply('{"reason":"x"}'), ["yes"]).ok).toBe(false);
  });
});

describe("fieldInReport", () => {
  it("defaults to on for strings and off for the rest", () => {
    expect(fieldInReport({ type: "string" })).toBe(true);
    expect(fieldInReport({ type: "number" })).toBe(false);
    expect(fieldInReport({ type: "boolean" })).toBe(false);
  });

  it("follows an explicit inReport", () => {
    expect(fieldInReport({ type: "string", inReport: false })).toBe(false);
    expect(fieldInReport({ type: "number", inReport: true })).toBe(true);
  });
});
