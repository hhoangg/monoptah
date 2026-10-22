import { describe, expect, it } from "vitest";
import type { Workflow, WorkflowNode } from "./workflow";
import { newWorkflowId, triggerOf } from "./workflow";
import { parseWorkflows, validateWorkflow } from "./workflowParse";

/** The ClickUp -> Dev -> Review example from the design spec (section 9.1), as stored. */
function example(): Workflow {
  return {
    id: "wf-abcd1234",
    name: "ClickUp -> Dev -> Review",
    description: "Pull my ClickUp tickets, implement each, review up to 3 rounds.",
    inputs: {
      project: { type: "project" },
      clickupList: { type: "string", description: "ClickUp list to pull from" },
    },
    inputValues: { project: "/Users/x/repo", clickupList: "Sprint" },
    nodes: [
      {
        id: "trigger",
        type: "schedule",
        schedule: { scheduleKind: "weekdays", time: "09:00", minute: 0, dayOfWeek: 1 },
        ui: { x: 0, y: 0 },
      },
      {
        id: "fetch",
        type: "session",
        project: "{{inputs.project}}",
        model: "sonnet",
        checkout: "root",
        prompt: "List open tickets assigned to me in {{inputs.clickupList}}.",
        output: { tickets: { type: "list" } },
      },
      { id: "each", type: "forEach", in: "{{fetch.tickets}}", as: "item", maxParallel: 3 },
      {
        id: "dev",
        type: "session",
        parent: "each",
        project: "{{inputs.project}}",
        model: "sonnet",
        checkout: "newWorktree",
        prompt:
          "Implement {{item.title}} ({{item.url}}) and open a PR.\n" +
          "{{#if review}}Fix these review issues: {{review.issues}}{{/if}}",
        output: { pr_url: { type: "string" }, summary: { type: "string" } },
      },
      {
        id: "review",
        type: "session",
        parent: "each",
        project: "{{inputs.project}}",
        model: "opus",
        checkout: { worktreeOf: "dev" },
        nestUnder: "dev",
        prompt: "Review {{dev.pr_url}}. List blocking issues only.",
        output: { verdict: { type: "string" }, issues: { type: "list" } },
      },
      { id: "report", type: "report" },
    ],
    edges: [
      { from: "trigger", to: "fetch" },
      { from: "fetch", to: "each" },
      { from: "dev", to: "review" },
      { from: "each", to: "report" },
      { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
    ],
    enabled: true,
    createdAt: 1_700_000_000_000,
  };
}

function messages(w: Workflow): string[] {
  return validateWorkflow(w).map((i) => i.message);
}

function flags(w: Workflow, phrase: string): boolean {
  return validateWorkflow(w).some((i) => i.message.includes(phrase));
}

function patchNode(w: Workflow, id: string, patch: Record<string, unknown>): Workflow {
  return {
    ...w,
    nodes: w.nodes.map((n) => (n.id === id ? ({ ...n, ...patch } as WorkflowNode) : n)),
  };
}

/** The example plus a command node run right after `fetch`. */
function withCommand(command: string): Workflow {
  const w = example();
  return {
    ...w,
    nodes: [
      ...w.nodes,
      { id: "lint", type: "command", project: "{{inputs.project}}", command },
    ],
    edges: [...w.edges, { from: "fetch", to: "lint" }],
  };
}

describe("parseWorkflows", () => {
  it("parses a valid ClickUp workflow", () => {
    const w = example();
    expect(parseWorkflows(JSON.stringify([w]))).toEqual([w]);
  });

  it("drops entries without id or nodes", () => {
    expect(parseWorkflows('[{"name":"x"}]')).toEqual([]);
    expect(parseWorkflows('[{"id":"wf-aaaaaaaa","name":"x"}]')).toEqual([]);
    expect(parseWorkflows('[{"nodes":[]}]')).toEqual([]);
    expect(parseWorkflows("not json")).toEqual([]);
    expect(parseWorkflows("")).toEqual([]);
    expect(parseWorkflows('{"id":"wf-aaaaaaaa"}')).toEqual([]);
  });

  it("keeps good entries next to malformed ones", () => {
    const good = example();
    const text = JSON.stringify([null, 4, { id: "wf-zzzzzzzz", nodes: [{ id: "a" }] }, good]);
    expect(parseWorkflows(text)).toEqual([good]);
  });

  it("fills defaults for a bare entry", () => {
    const [w] = parseWorkflows('[{"id":"wf-aaaaaaaa","nodes":[{"id":"t","type":"manual"}]}]');
    expect(w).toMatchObject({
      id: "wf-aaaaaaaa",
      name: "",
      inputs: {},
      inputValues: {},
      edges: [],
      enabled: false,
      createdAt: 0,
    });
  });
});

describe("workflow ids", () => {
  it("newWorkflowId is wf- plus 8 base36 chars", () => {
    expect(newWorkflowId()).toMatch(/^wf-[a-z0-9]{8}$/);
  });

  it("triggerOf finds the trigger", () => {
    expect(triggerOf(example())?.id).toBe("trigger");
  });
});

describe("validateWorkflow", () => {
  it("accepts loop review -> dev inside forEach", () => {
    expect(validateWorkflow(example())).toEqual([]);
  });

  it("flags two triggers", () => {
    const w = example();
    w.nodes.push({ id: "again", type: "manual" });
    expect(flags(w, "exactly one trigger")).toBe(true);
  });

  it("flags no trigger", () => {
    const w = example();
    w.nodes = w.nodes.filter((n) => n.id !== "trigger");
    w.edges = w.edges.filter((e) => e.from !== "trigger");
    expect(flags(w, "exactly one trigger")).toBe(true);
  });

  it("flags missing report", () => {
    const w = example();
    w.nodes = w.nodes.filter((n) => n.id !== "report");
    w.edges = w.edges.filter((e) => e.to !== "report");
    expect(flags(w, "exactly one Report node")).toBe(true);
  });

  it("flags two report nodes", () => {
    const w = example();
    w.nodes.push({ id: "report2", type: "report" });
    expect(flags(w, "exactly one Report node")).toBe(true);
  });

  it("flags a report the trigger cannot reach", () => {
    const w = example();
    w.edges = w.edges.filter((e) => e.to !== "report");
    expect(flags(w, "reachable from the trigger")).toBe(true);
  });

  it("flags a plain cycle without loop", () => {
    const w = example();
    w.edges.push({ from: "review", to: "dev" });
    expect(flags(w, "cycle")).toBe(true);
  });

  it("flags loop edge to a non-ancestor", () => {
    const w = example();
    w.edges = w.edges.filter((e) => !e.loop);
    w.edges.push({ from: "dev", to: "review", loop: { maxRounds: 3, decide: "mono" } });
    expect(flags(w, "ancestor")).toBe(true);
  });

  it("flags a loop edge that crosses a For each boundary", () => {
    const w = example();
    w.edges = w.edges.filter((e) => !e.loop);
    w.edges.push({ from: "review", to: "fetch", loop: { maxRounds: 3, decide: "mono" } });
    expect(flags(w, "same For each")).toBe(true);
  });

  it("flags a loop edge that starts at a node that cannot loop", () => {
    const w = example();
    w.edges = w.edges.filter((e) => !e.loop);
    w.edges.push({ from: "each", to: "fetch", loop: { maxRounds: 3, decide: "mono" } });
    expect(flags(w, "session, command or Ask Mono")).toBe(true);
    const t = example();
    t.edges = t.edges.filter((e) => !e.loop);
    t.edges.push({ from: "trigger", to: "trigger", loop: { maxRounds: 3, decide: "mono" } });
    expect(flags(t, "session, command or Ask Mono")).toBe(true);
  });

  it("flags a node that is the source of two loop edges", () => {
    const w = example();
    w.edges.push({ from: "review", to: "dev", loop: { maxRounds: 2, decide: "mono" } });
    expect(messages(w).filter((m) => m.includes("more than one loop edge"))).toHaveLength(1);
  });

  it("flags a Report node with outgoing edges or a parent", () => {
    const w = example();
    w.nodes.push({
      id: "after",
      type: "command",
      project: "{{inputs.project}}",
      command: "true",
    });
    w.edges.push({ from: "report", to: "after" });
    expect(flags(w, "leaves the Report node")).toBe(true);
    expect(flags(patchNode(example(), "report", { parent: "each" }), "cannot be inside a For each")).toBe(true);
  });

  it("flags maxRounds 11", () => {
    const w = example();
    w.edges = w.edges.map((e) => (e.loop ? { ...e, loop: { maxRounds: 11, decide: "mono" } } : e));
    expect(flags(w, "maxRounds")).toBe(true);
  });

  it("flags maxParallel out of range", () => {
    expect(flags(patchNode(example(), "each", { maxParallel: 11 }), "maxParallel")).toBe(true);
    expect(flags(patchNode(example(), "each", { maxParallel: 0 }), "maxParallel")).toBe(true);
  });

  it("flags unknown {{dev.nope}}", () => {
    const w = patchNode(example(), "review", { prompt: "Review {{dev.nope}}." });
    expect(flags(w, "{{dev.nope}}")).toBe(true);
  });

  it("flags a variable that names no node, input or item", () => {
    const w = patchNode(example(), "fetch", { prompt: "Hello {{ghost.value}}" });
    expect(flags(w, "{{ghost.value}}")).toBe(true);
  });

  it("flags an undeclared input", () => {
    const w = patchNode(example(), "fetch", { prompt: "List {{inputs.nope}}" });
    expect(flags(w, "{{inputs.nope}}")).toBe(true);
  });

  it("flags a node that is not upstream", () => {
    const w = patchNode(example(), "fetch", { prompt: "Use {{dev.pr_url}}" });
    expect(flags(w, "not upstream")).toBe(true);
  });

  it("flags item outside a For each", () => {
    const w = patchNode(example(), "fetch", { prompt: "Use {{item.title}}" });
    expect(flags(w, "{{item.title}}")).toBe(true);
  });

  it("uses the For each 'as' name for the loop variable", () => {
    let w = patchNode(example(), "each", { as: "ticket" });
    expect(flags(w, "{{item.title}}")).toBe(true);
    w = patchNode(w, "dev", {
      prompt: "Implement {{ticket.title}}.\n{{#if review}}{{review.issues}}{{/if}}",
    });
    expect(validateWorkflow(w)).toEqual([]);
  });

  it("accepts run.round, run.id and run.startedAt anywhere", () => {
    const w = patchNode(example(), "fetch", {
      prompt: "Round {{run.round}} of {{run.id}} started {{run.startedAt}}",
    });
    expect(validateWorkflow(w)).toEqual([]);
    expect(flags(patchNode(example(), "fetch", { prompt: "{{run.nope}}" }), "{{run.nope}}")).toBe(
      true,
    );
  });

  it("resolves command outputs, askMono outputs and forEach results", () => {
    const w = example();
    w.nodes.push(
      { id: "lint", type: "command", project: "{{inputs.project}}", command: "make lint" },
      { id: "ask", type: "askMono", question: "Exit {{lint.exitCode}}?", answers: ["yes", "no"] },
    );
    w.edges.push(
      { from: "fetch", to: "lint" },
      { from: "lint", to: "ask" },
      { from: "ask", to: "report" },
    );
    const report = w.nodes.find((n) => n.id === "report");
    if (report?.type === "report") {
      report.template = "{{ask.answer}} {{ask.reason}} {{each.results}} {{lint.stderr}}";
    }
    expect(validateWorkflow(w)).toEqual([]);
    expect(flags(patchNode(w, "ask", { question: "{{lint.nope}}" }), "{{lint.nope}}")).toBe(true);
  });

  it("checks variables in if conditions, forEach 'in' and loop deciders", () => {
    expect(flags(patchNode(example(), "each", { in: "{{fetch.nope}}" }), "{{fetch.nope}}")).toBe(
      true,
    );
    const w = example();
    w.nodes.push({ id: "gate", type: "if", condition: '{{ghost.x}} == "a"' });
    w.edges.push({ from: "fetch", to: "gate" });
    expect(flags(w, "{{ghost.x}}")).toBe(true);
    const looped = example();
    looped.edges = looped.edges.map((e) =>
      e.loop ? { ...e, loop: { maxRounds: 3, decide: '{{review.nope}} == "x"' } } : e,
    );
    expect(flags(looped, "{{review.nope}}")).toBe(true);
    looped.edges = looped.edges.map((e) =>
      e.loop ? { ...e, loop: { maxRounds: 3, decide: '{{review.verdict}} != "approve"' } } : e,
    );
    expect(validateWorkflow(looped)).toEqual([]);
  });

  it('flags absolute project path "/Users/x/repo"', () => {
    const w = patchNode(example(), "fetch", { project: "/Users/x/repo" });
    expect(flags(w, "absolute path")).toBe(true);
  });

  it("flags a project that is not a project input", () => {
    expect(flags(patchNode(example(), "fetch", { project: "repo" }), "{{inputs.")).toBe(true);
    expect(
      flags(patchNode(example(), "fetch", { project: "{{inputs.clickupList}}" }), "project input"),
    ).toBe(true);
    expect(
      flags(patchNode(example(), "fetch", { project: "{{inputs.nope}}" }), "project input"),
    ).toBe(true);
  });

  it("flags absolute paths in other node fields", () => {
    const w = patchNode(example(), "fetch", { prompt: "Read /Users/x/repo/notes.md first" });
    expect(flags(w, "absolute path")).toBe(true);
    expect(flags(withCommand("cat ~/secrets"), "absolute path")).toBe(true);
    expect(
      flags(patchNode(example(), "fetch", { prompt: "GET /api/users and https://x.dev/home/a" }), "absolute path"),
    ).toBe(false);
  });

  it("flags if without true/false handles", () => {
    const w = example();
    w.nodes.push({ id: "gate", type: "if", condition: '{{fetch.tickets}} not empty' });
    w.edges.push({ from: "fetch", to: "gate" }, { from: "gate", to: "report" });
    expect(flags(w, "handle")).toBe(true);
    w.edges = w.edges.map((e) => (e.from === "gate" ? { ...e, handle: "true" as const } : e));
    expect(validateWorkflow(w)).toEqual([]);
  });

  it("flags an if with both or neither of condition and decide", () => {
    const base = example();
    base.nodes.push({ id: "gate", type: "if" });
    base.edges.push({ from: "fetch", to: "gate" });
    expect(flags(base, "condition or decide")).toBe(true);
    const both = patchNode(base, "gate", { condition: "a == b", decide: "mono" });
    expect(flags(both, "condition or decide")).toBe(true);
    expect(validateWorkflow(patchNode(base, "gate", { decide: "mono" }))).toEqual([]);
  });

  it("flags a handle on an edge that does not leave an if", () => {
    const w = example();
    w.edges = w.edges.map((e) => (e.from === "fetch" ? { ...e, handle: "true" as const } : e));
    expect(flags(w, "handle")).toBe(true);
  });

  it("flags duplicate and malformed node ids", () => {
    const dup = example();
    dup.nodes.push({ id: "fetch", type: "manual" });
    expect(flags(dup, "Duplicate node id")).toBe(true);
    const bad = patchNode(example(), "fetch", { id: "Fetch-1" });
    expect(flags(bad, "node id")).toBe(true);
  });

  it("flags edges with a missing endpoint", () => {
    const w = example();
    w.edges.push({ from: "fetch", to: "ghost" });
    expect(flags(w, "ghost")).toBe(true);
  });

  it("flags a parent that is not a For each", () => {
    expect(flags(patchNode(example(), "dev", { parent: "fetch" }), "For each")).toBe(true);
    expect(flags(patchNode(example(), "dev", { parent: "ghost" }), "ghost")).toBe(true);
  });

  it("flags references to missing session nodes", () => {
    expect(
      flags(patchNode(example(), "review", { checkout: { worktreeOf: "ghost" } }), "ghost"),
    ).toBe(true);
    expect(flags(patchNode(example(), "review", { nestUnder: "ghost" }), "ghost")).toBe(true);
  });

  it("flags an invalid schedule", () => {
    const w = patchNode(example(), "trigger", {
      schedule: { scheduleKind: "monthly", time: "9am", minute: 0, dayOfWeek: 1 },
    });
    expect(flags(w, "schedule")).toBe(true);
  });

  it("accepts an interval schedule", () => {
    const w = patchNode(example(), "trigger", {
      schedule: {
        scheduleKind: "interval",
        time: "00:00",
        minute: 0,
        dayOfWeek: 1,
        intervalHours: 6,
        intervalFrom: "2026-01-01",
      },
    });
    expect(validateWorkflow(w)).toEqual([]);
  });

  it("flags input names that cannot be used in a variable", () => {
    const w = example();
    w.inputs["bad.name"] = { type: "string" };
    expect(flags(w, "input name")).toBe(true);
  });

  it("flags an empty askMono answer list and an empty forEach list expression", () => {
    const w = example();
    w.nodes.push({ id: "ask", type: "askMono", question: "ok?", answers: [] });
    w.edges.push({ from: "fetch", to: "ask" });
    expect(flags(w, "answer")).toBe(true);
    expect(flags(patchNode(example(), "each", { in: "" }), "input list")).toBe(true);
  });

  it("never throws on a workflow with odd fields", () => {
    const w = example();
    w.edges.push({ from: "each", to: "each" });
    expect(() => validateWorkflow(w)).not.toThrow();
    expect(flags(w, "cycle")).toBe(true);
  });
});

describe("validateWorkflow absolute paths in commands", () => {
  it.each([
    "make >/tmp/log",
    "make 2>/var/log/x",
    "make;/usr/bin/x",
    "make|/usr/bin/x",
    "cd /srv/app && make",
    "cd /tmp",
    "make &>/srv/out",
    "cat </etc/hosts",
    "make --out=/srv/out",
    "cd ~/work && make",
  ])("flags %s", (command) => {
    expect(flags(withCommand(command), "absolute path")).toBe(true);
  });

  it("does not flag URLs, relative paths or discarded output", () => {
    for (const command of [
      "curl https://example.com/a/b {{inputs.clickupList}}",
      "curl -s http://localhost:3000/health",
      "git commit -m {{inputs.clickupList}}",
      "make 2>/dev/null",
      "make >/dev/null 2>&1",
      "ls ./src/features && cat docs/a.md",
      "echo $((6 / 3))",
    ]) {
      expect(flags(withCommand(command), "absolute path")).toBe(false);
    }
  });

  it("flags a path only after shell operators in prose prompts too", () => {
    const w = patchNode(example(), "fetch", { prompt: "Write the log to >/tmp/log" });
    expect(flags(w, "absolute path")).toBe(true);
  });
});

describe("prototype-like map keys", () => {
  it("parseWorkflows skips a __proto__ input or output key", () => {
    const text =
      '[{"id":"wf-aaaaaaaa","nodes":[{"id":"s","type":"session","project":"p","prompt":"x",' +
      '"checkout":"root","output":{"__proto__":{"type":"string"},"ok":{"type":"string"}}}],' +
      '"inputs":{"__proto__":{"type":"project"},"p":{"type":"project"}},' +
      '"inputValues":{"__proto__":"x","p":"y"}}]';
    const [w] = parseWorkflows(text);
    expect(Object.keys(w.inputs)).toEqual(["p"]);
    expect(Object.getPrototypeOf(w.inputs)).toBe(Object.prototype);
    expect(Object.keys(w.inputValues)).toEqual(["p"]);
    const session = w.nodes[0];
    expect(session.type === "session" && Object.keys(session.output)).toEqual(["ok"]);
    expect(session.type === "session" && Object.getPrototypeOf(session.output)).toBe(
      Object.prototype,
    );
  });

  it("a project that names an inherited key is not a project input", () => {
    for (const name of ["__proto__", "constructor", "toString"]) {
      const w = patchNode(example(), "fetch", { project: `{{inputs.${name}}}` });
      expect(flags(w, "project input")).toBe(true);
    }
  });

  it("flags a __proto__ input name", () => {
    const w = example();
    Object.defineProperty(w.inputs, "__proto__", {
      value: { type: "string" },
      enumerable: true,
    });
    expect(flags(w, "input name")).toBe(true);
  });
});

describe("validateWorkflow command templates", () => {
  it("flags a variable inside double quotes", () => {
    const issues = validateWorkflow(withCommand('echo "{{inputs.clickupList}}"'));
    expect(issues.some((i) => i.nodeId === "lint" && i.message.includes("double quotes"))).toBe(
      true,
    );
  });

  it("accepts a bare variable", () => {
    expect(validateWorkflow(withCommand("echo {{inputs.clickupList}}"))).toEqual([]);
  });

  it("flags unquotable shell syntax and conditionals", () => {
    expect(messages(withCommand("echo $(date) {{inputs.clickupList}}")).length).toBeGreaterThan(0);
    expect(
      flags(withCommand("{{#if inputs.clickupList}}make{{/if}}"), "conditionals"),
    ).toBe(true);
  });

  it("still resolves variables in a command", () => {
    expect(flags(withCommand("echo {{ghost.x}}"), "{{ghost.x}}")).toBe(true);
    expect(validateWorkflow(withCommand("echo {{fetch.tickets}}"))).toEqual([]);
  });
});
