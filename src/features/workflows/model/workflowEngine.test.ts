import { describe, expect, it } from "vitest";
import type { Workflow, WorkflowEdge, WorkflowNode } from "./workflow";
import {
  applyEvent,
  loopedNodeIds,
  markInterrupted,
  nextActions,
  resumeRun,
  runScope,
  startRun,
  type EngineAction,
  type EngineEvent,
  type WorkflowRun,
} from "./workflowEngine";

const PROJECT = "{{inputs.project}}";

function def(nodes: WorkflowNode[], edges: WorkflowEdge[]): Workflow {
  return {
    id: "wf-aaaaaaaa",
    name: "Test",
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
  return {
    id,
    type: "session",
    project: PROJECT,
    checkout: "root",
    prompt: `Do ${id}`,
    output: {},
    ...extra,
  };
}

/** trigger -> fetch (session with a tickets list) -> each (forEach over it) -> report. */
function fanOut(opts: { maxParallel?: number; children?: WorkflowNode[]; edges?: WorkflowEdge[] } = {}) {
  return def(
    [
      trigger,
      session("fetch", { output: { tickets: { type: "list" } } }),
      {
        id: "each",
        type: "forEach",
        in: "{{fetch.tickets}}",
        as: "item",
        maxParallel: opts.maxParallel ?? 2,
      },
      ...(opts.children ?? [
        session("dev", {
          parent: "each",
          prompt: "Implement {{item.title}}",
          output: { pr_url: { type: "string" } },
        }),
      ]),
      report,
    ],
    [
      { from: "trigger", to: "fetch" },
      { from: "fetch", to: "each" },
      { from: "each", to: "report" },
      ...(opts.edges ?? []),
    ],
  );
}

/** trigger -> dev -> review (loop back to dev) -> report. */
function devReview(decide: string, maxRounds = 3, reuseSession?: boolean) {
  return def(
    [
      trigger,
      session("dev", {
        prompt: "Implement round {{run.round}}.{{#if review}} Fix: {{review.issues}}{{/if}}",
        output: { pr_url: { type: "string" } },
        ...(reuseSession === undefined ? {} : { reuseSession }),
      }),
      session("review", { prompt: "Review {{dev.pr_url}}", output: { verdict: { type: "string" } } }),
      report,
    ],
    [
      { from: "trigger", to: "dev" },
      { from: "dev", to: "review" },
      { from: "review", to: "report" },
      { from: "review", to: "dev", loop: { maxRounds, decide } },
    ],
  );
}

const at = 1_000;

function started(run: WorkflowRun, nodeId: string, branch = "", sessionId?: string, time = at) {
  return applyEvent(run, { kind: "nodeStarted", nodeId, branch, ...(sessionId ? { sessionId } : {}), at: time });
}
function finished(run: WorkflowRun, nodeId: string, output: Record<string, unknown>, branch = "", time = at) {
  return applyEvent(run, { kind: "nodeFinished", nodeId, branch, output, at: time });
}
function failed(run: WorkflowRun, nodeId: string, error: string, branch = "", time = at) {
  return applyEvent(run, { kind: "nodeFailed", nodeId, branch, error, at: time });
}
function decided(run: WorkflowRun, nodeId: string, value: string, branch = "", reason?: string) {
  return applyEvent(run, {
    kind: "decided",
    nodeId,
    branch,
    value,
    ...(reason ? { reason } : {}),
    at,
  });
}
/** Starts and finishes a node in one go. */
function complete(run: WorkflowRun, nodeId: string, output: Record<string, unknown>, branch = "", time = at) {
  return finished(started(run, nodeId, branch, undefined, time), nodeId, output, branch, time);
}
function statusOf(run: WorkflowRun, nodeId: string, branch = "") {
  return run.nodes.find((n) => n.nodeId === nodeId && n.branch === branch)?.status;
}
function nodeOf(run: WorkflowRun, nodeId: string, branch = "") {
  const n = run.nodes.find((x) => x.nodeId === nodeId && x.branch === branch);
  if (!n) throw new Error(`no node run ${nodeId}/${branch}`);
  return n;
}
const sessions = (actions: EngineAction[]) => actions.filter((a) => a.kind === "startSession");

const TICKETS = [{ title: "A" }, { title: "B" }, { title: "C" }];

describe("workflowEngine", () => {
  it("linear trigger->fetch->report emits startSession then postReport then done", () => {
    let run = startRun(
      def([trigger, session("fetch", { prompt: "Fetch for {{inputs.project}}" }), report], [
        { from: "trigger", to: "fetch" },
        { from: "fetch", to: "report" },
      ]),
      "manual",
      at,
      "run-1",
    );
    expect(statusOf(run, "trigger")).toBe("done");
    expect(nextActions(run)).toEqual([
      { kind: "startSession", nodeId: "fetch", branch: "", round: 1, prompt: "Fetch for /work/repo" },
    ]);
    run = started(run, "fetch", "", "s-1");
    expect(nextActions(run)).toEqual([]);
    run = finished(run, "fetch", {});
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    expect(run.status).toBe("running");
    run = applyEvent(run, { kind: "reportPosted", at: 2_000 });
    expect(run.status).toBe("done");
    expect(run.reportPostedAt).toBe(2_000);
    expect(run.endedAt).toBe(2_000);
    expect(nextActions(run)).toEqual([]);
  });

  it("schedule trigger exposes startedAt", () => {
    const run = startRun(
      def(
        [
          { id: "trigger", type: "schedule", schedule: { scheduleKind: "hourly", time: "00:00", minute: 0, dayOfWeek: 1 } },
          report,
        ],
        [{ from: "trigger", to: "report" }],
      ),
      "schedule",
      5,
      "r",
    );
    expect(nodeOf(run, "trigger").output).toEqual({ startedAt: 5 });
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
  });

  it("appends the output instruction to the prompt only when fields are declared", () => {
    const run = startRun(
      def([trigger, session("a", { output: { x: { type: "string" } } }), report], [
        { from: "trigger", to: "a" },
        { from: "a", to: "report" },
      ]),
      "manual",
      at,
      "r",
    );
    const [action] = sessions(nextActions(run));
    expect(action.kind === "startSession" && action.prompt).toMatch(/^Do a\n\nWhen you are finished/);
    const bare = startRun(
      def([trigger, session("a"), report], [
        { from: "trigger", to: "a" },
        { from: "a", to: "report" },
      ]),
      "manual",
      at,
      "r",
    );
    expect(nextActions(bare)[0]).toMatchObject({ prompt: "Do a" });
  });

  it("renders commands with shell quoting and fails unsafe command templates", () => {
    const ok = startRun(
      def(
        [
          trigger,
          session("a", { output: { name: { type: "string" } } }),
          { id: "cmd", type: "command", project: PROJECT, command: "echo {{a.name}}" },
          report,
        ],
        [
          { from: "trigger", to: "a" },
          { from: "a", to: "cmd" },
          { from: "cmd", to: "report" },
        ],
      ),
      "manual",
      at,
      "r",
    );
    const next = complete(ok, "a", { name: "it's" });
    expect(nextActions(next)).toEqual([
      { kind: "runCommand", nodeId: "cmd", branch: "", command: `echo 'it'\\''s'` },
    ]);

    const unsafe = startRun(
      def(
        [
          trigger,
          session("a", { output: { name: { type: "string" } } }),
          { id: "cmd", type: "command", project: PROJECT, command: `echo "{{a.name}}"` },
          report,
        ],
        [
          { from: "trigger", to: "a" },
          { from: "a", to: "cmd" },
          { from: "cmd", to: "report" },
        ],
      ),
      "manual",
      at,
      "r",
    );
    const after = complete(unsafe, "a", { name: "x" });
    expect(statusOf(after, "cmd")).toBe("failed");
    expect(nodeOf(after, "cmd").error).toContain("inside double quotes");
    expect(nextActions(after)).toEqual([{ kind: "postReport" }]);
  });

  it("fails a command whose rendered value would be read as an option", () => {
    const cmd = (command: string) =>
      startRun(
        def(
          [
            trigger,
            session("a", { output: { host: { type: "string" } } }),
            { id: "cmd", type: "command", project: PROJECT, command },
            report,
          ],
          [
            { from: "trigger", to: "a" },
            { from: "a", to: "cmd" },
            { from: "cmd", to: "report" },
          ],
        ),
        "manual",
        at,
        "r",
      );
    const bad = complete(cmd("ssh {{a.host}}"), "a", { host: "-oProxyCommand=x" });
    expect(statusOf(bad, "cmd")).toBe("failed");
    expect(nodeOf(bad, "cmd").error).toContain("{{a.host}}");
    expect(nextActions(bad)).toEqual([{ kind: "postReport" }]);

    const guarded = complete(cmd("git log -- {{a.host}}"), "a", { host: "-x" });
    expect(nextActions(guarded)).toEqual([
      { kind: "runCommand", nodeId: "cmd", branch: "", command: "git log -- '-x'" },
    ]);
    const plain = complete(cmd("ssh {{a.host}}"), "a", { host: "example.com" });
    expect(nextActions(plain)).toEqual([
      { kind: "runCommand", nodeId: "cmd", branch: "", command: "ssh 'example.com'" },
    ]);
  });

  it("forEach fans out 3 items with maxParallel 2 (2 then 1)", () => {
    let run = startRun(fanOut(), "manual", at, "r");
    run = complete(run, "fetch", { tickets: TICKETS });
    expect(run.items.each).toEqual(TICKETS);
    expect(statusOf(run, "each")).toBe("running");

    const first = nextActions(run);
    expect(first.map((a) => a.kind === "startSession" && [a.nodeId, a.branch, a.prompt])).toEqual([
      ["dev", "each#0", expect.stringContaining("Implement A")],
      ["dev", "each#1", expect.stringContaining("Implement B")],
    ]);

    run = started(run, "dev", "each#0");
    run = started(run, "dev", "each#1");
    expect(nextActions(run)).toEqual([]);

    run = finished(run, "dev", { pr_url: "u0" }, "each#0");
    const second = nextActions(run);
    expect(second).toHaveLength(1);
    expect(second[0]).toMatchObject({ nodeId: "dev", branch: "each#2" });

    run = started(run, "dev", "each#2");
    run = finished(run, "dev", { pr_url: "u1" }, "each#1");
    run = finished(run, "dev", { pr_url: "u2" }, "each#2");
    expect(statusOf(run, "each")).toBe("done");
    expect(nodeOf(run, "each").output).toEqual({
      results: [{ pr_url: "u0" }, { pr_url: "u1" }, { pr_url: "u2" }],
    });
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
  });

  it("forEach empty list completes with no branches", () => {
    let run = startRun(fanOut(), "manual", at, "r");
    run = complete(run, "fetch", { tickets: [] });
    expect(statusOf(run, "each")).toBe("done");
    expect(nodeOf(run, "each").output).toEqual({ results: [] });
    expect(run.nodes.filter((n) => n.branch !== "")).toEqual([]);
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    run = applyEvent(run, { kind: "reportPosted", at: 3 });
    expect(run.status).toBe("done");
  });

  it('forEach on non-list fails with "Expected a list"', () => {
    let run = startRun(fanOut(), "manual", at, "r");
    run = complete(run, "fetch", { tickets: "nope" });
    expect(statusOf(run, "each")).toBe("failed");
    expect(nodeOf(run, "each").error).toBe("Expected a list for {{fetch.tickets}}");
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    run = applyEvent(run, { kind: "reportPosted", at: 3 });
    expect(run.status).toBe("doneWithFailures");
  });

  it("fails a nested forEach and ignores edges that cross a For each boundary", () => {
    const nested = fanOut({
      children: [
        session("dev", { parent: "each" }),
        { id: "inner", type: "forEach", parent: "each", in: "{{item.sub}}" },
      ],
      edges: [
        { from: "dev", to: "inner" },
        // Crossing edge: ignored, so dev still starts without waiting for fetch's edge.
        { from: "fetch", to: "dev" },
      ],
    });
    let run = startRun(nested, "manual", at, "r");
    run = complete(run, "fetch", { tickets: [{ sub: [1] }] });
    run = complete(run, "dev", {}, "each#0");
    expect(statusOf(run, "inner", "each#0")).toBe("failed");
    expect(nodeOf(run, "inner", "each#0").error).toBe("Nested For each is not supported");
    expect(statusOf(run, "each")).toBe("done");
  });

  it("loop asks Mono and resets dev+review to round 2 with reuseSessionId", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = started(run, "dev", "", "s-dev");
    run = finished(run, "dev", { pr_url: "pr1" });
    run = started(run, "review");
    run = finished(run, "review", { verdict: "changes", issues: ["x"] });
    // review has finished but is not settled until the Mono decides.
    expect(statusOf(run, "review")).toBe("done");
    expect(nextActions(run)).toEqual([
      expect.objectContaining({
        kind: "askMono",
        nodeId: "review",
        purpose: "loop",
        answers: ["loop", "exit"],
        context: expect.objectContaining({ review: { verdict: "changes", issues: ["x"] }, round: 1 }),
      }),
    ]);
    // The ask is repeated until it is acknowledged, then not again.
    run = started(run, "review");
    expect(nextActions(run)).toEqual([]);

    run = decided(run, "review", "loop", "", "issues remain");
    expect(statusOf(run, "dev")).toBe("pending");
    expect(statusOf(run, "review")).toBe("pending");
    expect(nodeOf(run, "dev").round).toBe(2);
    expect(nodeOf(run, "review").round).toBe(2);
    expect(nodeOf(run, "dev").output).toEqual({ pr_url: "pr1" }); // kept until it reruns
    const [again] = nextActions(run);
    expect(again).toMatchObject({
      kind: "startSession",
      nodeId: "dev",
      round: 2,
      reuseSessionId: "s-dev",
    });
    expect(again.kind === "startSession" && again.prompt).toContain('Implement round 2. Fix: [\n  "x"\n]');
  });

  it("records the finished round before a loop resets a node", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = started(run, "dev", "", "s-dev-1");
    run = finished(run, "dev", { pr_url: "u1" });
    run = started(run, "review", "", "s-rev-1");
    run = finished(run, "review", { verdict: "changes" });
    run = decided(run, "review", "loop", "", "needs tests");
    expect(nodeOf(run, "dev").history).toEqual([
      expect.objectContaining({
        round: 1,
        status: "done",
        sessionId: "s-dev-1",
      }),
    ]);
    // Keeping each round's output would multiply the size of a run, and nothing shows it.
    expect(nodeOf(run, "dev").history?.[0]).not.toHaveProperty("output");
    expect(nodeOf(run, "review").history).toHaveLength(1);
    expect(nodeOf(run, "review").history?.[0].sessionId).toBe("s-rev-1");
    expect(nodeOf(run, "review").history?.[0].decision).toEqual({ value: "loop", reason: "needs tests" });
    expect(nodeOf(run, "review").round).toBe(2);
    // The current round is never part of the history.
    expect(nodeOf(run, "review").decision).toBeUndefined();
  });

  it("exit decision stays on the node, not in history", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p1" });
    run = complete(run, "review", { verdict: "no" });
    run = decided(run, "review", "loop");
    run = complete(run, "dev", { pr_url: "p2" });
    run = complete(run, "review", { verdict: "ok" });
    run = decided(run, "review", "exit");
    expect(nodeOf(run, "review").decision?.value).toBe("exit");
    expect(nodeOf(run, "review").history).toHaveLength(1);
    expect(nodeOf(run, "review").history?.[0].round).toBe(1);
  });

  it("loop does not reuse the session when reuseSession is false", () => {
    let run = startRun(devReview("mono", 3, false), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p" });
    run = complete(run, "review", { verdict: "no" });
    run = decided(run, "review", "loop");
    const [action] = nextActions(run);
    expect(action).toMatchObject({ kind: "startSession", nodeId: "dev", round: 2 });
    expect(action).not.toHaveProperty("reuseSessionId");
  });

  it('loop stops at maxRounds with reason "max rounds reached"', () => {
    let run = startRun(devReview("mono", 2), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p1" });
    run = complete(run, "review", { verdict: "no" });
    run = decided(run, "review", "loop");
    run = complete(run, "dev", { pr_url: "p2" });
    run = complete(run, "review", { verdict: "no" });
    expect(nodeOf(run, "review").decision).toEqual({ value: "exit", reason: "max rounds reached" });
    // No question is put to the Mono; the run moves on to the report.
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
  });

  it("loop exits when the Mono answers exit", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p1" });
    run = complete(run, "review", { verdict: "ok" });
    run = started(run, "review");
    run = decided(run, "review", "exit", "", "looks good");
    expect(statusOf(run, "review")).toBe("done");
    expect(nodeOf(run, "review").decision).toEqual({ value: "exit", reason: "looks good" });
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
  });

  it("condition loops evaluate without asking the Mono", () => {
    let run = startRun(devReview('{{review.verdict}} != "approve"'), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p1" });
    run = complete(run, "review", { verdict: "changes" });
    expect(nodeOf(run, "dev").round).toBe(2);
    expect(nextActions(run)).toEqual([expect.objectContaining({ kind: "startSession", nodeId: "dev" })]);
    run = complete(run, "dev", { pr_url: "p2" });
    run = complete(run, "review", { verdict: "approve" });
    expect(nodeOf(run, "review").decision).toEqual({ value: "exit" });
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
  });

  it("condition loops record the loop decision in history", () => {
    let run = startRun(devReview('{{review.verdict}} != "approve"'), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p1" });
    run = complete(run, "review", { verdict: "changes" });
    expect(nodeOf(run, "review").history).toEqual([
      expect.objectContaining({ round: 1, status: "done", decision: { value: "loop" } }),
    ]);
    expect(nodeOf(run, "review").decision).toBeUndefined();
  });

  it("a round record leaves out what the node did in an earlier round", () => {
    const w = def(
      [
        trigger,
        session("dev"),
        { id: "gate", type: "if", condition: "{{dev.ok}} == true" },
        session("fixA"),
        session("fixB"),
        session("review"),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "gate" },
        { from: "gate", to: "fixA", handle: "true" },
        { from: "gate", to: "fixB", handle: "false" },
        { from: "fixA", to: "review" },
        { from: "fixB", to: "review" },
        { from: "review", to: "report" },
        { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
      ],
    );
    let run = complete(startRun(w, "manual", at, "r"), "dev", { ok: true });
    run = started(run, "fixA", "", "s-a");
    run = finished(run, "fixA", { fixed: "a" });
    run = complete(run, "review", {});
    run = decided(run, "review", "loop");
    run = complete(run, "dev", { ok: false });
    run = complete(run, "fixB", { fixed: "b" });
    run = complete(run, "review", {});
    run = decided(run, "review", "loop");
    const [a1, a2] = nodeOf(run, "fixA").history ?? [];
    expect(a1).toMatchObject({ round: 1, status: "done", sessionId: "s-a" });
    expect(a1).not.toHaveProperty("output");
    expect(a2).toMatchObject({ round: 2, status: "skipped" });
    expect(a2).not.toHaveProperty("sessionId");
    expect(a2).not.toHaveProperty("output");
    const [b1, b2] = nodeOf(run, "fixB").history ?? [];
    expect(b1).toMatchObject({ round: 1, status: "skipped" });
    expect(b1).not.toHaveProperty("output");
    expect(b2).toMatchObject({ round: 2, status: "done" });
    expect(b2).not.toHaveProperty("output");
  });

  it("loops inside a For each keep their rounds per branch", () => {
    const w = fanOut({
      maxParallel: 2,
      children: [
        session("dev", { parent: "each", prompt: "Round {{run.round}} of {{item.title}}", output: { pr_url: { type: "string" } } }),
        session("review", { parent: "each", output: { verdict: { type: "string" } } }),
      ],
      edges: [
        { from: "dev", to: "review" },
        { from: "review", to: "dev", loop: { maxRounds: 2, decide: '{{review.verdict}} != "approve"' } },
      ],
    });
    let run = complete(startRun(w, "manual", at, "r"), "fetch", { tickets: [{ title: "A" }, { title: "B" }] });
    run = complete(run, "dev", { pr_url: "a1" }, "each#0");
    run = complete(run, "review", { verdict: "changes" }, "each#0");
    run = complete(run, "dev", { pr_url: "b1" }, "each#1");
    run = complete(run, "review", { verdict: "approve" }, "each#1");
    expect(nodeOf(run, "dev", "each#0").round).toBe(2);
    expect(nodeOf(run, "dev", "each#1").round).toBe(1);
    const actions = nextActions(run);
    expect(actions).toHaveLength(1);
    expect(actions[0]).toMatchObject({ nodeId: "dev", branch: "each#0", round: 2 });
    expect(actions[0].kind === "startSession" && actions[0].prompt.startsWith("Round 2 of A")).toBe(true);
    run = complete(run, "dev", { pr_url: "a2" }, "each#0");
    run = complete(run, "review", { verdict: "changes" }, "each#0");
    expect(nodeOf(run, "review", "each#0").decision?.reason).toBe("max rounds reached");
    expect(statusOf(run, "each")).toBe("done");
  });

  it("if inside a loop body runs the other side in round 2", () => {
    const w = def(
      [
        trigger,
        session("dev"),
        { id: "gate", type: "if", condition: "{{dev.ok}} == true" },
        session("review"),
        session("escalate"),
        report,
      ],
      [
        { from: "trigger", to: "dev" },
        { from: "dev", to: "gate" },
        { from: "gate", to: "review", handle: "true" },
        { from: "gate", to: "escalate", handle: "false" },
        { from: "review", to: "report" },
        { from: "escalate", to: "report" },
        { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
      ],
    );
    let run = complete(startRun(w, "manual", at, "r"), "dev", { ok: true });
    expect(statusOf(run, "escalate")).toBe("skipped");
    run = complete(run, "review", {});
    run = decided(run, "review", "loop");
    expect(statusOf(run, "escalate")).toBe("pending");
    run = complete(run, "dev", { ok: false });
    expect(statusOf(run, "review")).toBe("skipped");
    expect(nextActions(run).map((a) => a.kind === "startSession" && a.nodeId)).toEqual(["escalate"]);
    run = complete(run, "escalate", {});
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    expect(statusOf(run, "escalate")).toBe("done");
  });

  it("does not mutate the input run after a loop reset or a For each expansion", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p" });
    run = complete(run, "review", { verdict: "no" });
    const before = JSON.stringify(run);
    const after = decided(run, "review", "loop");
    expect(JSON.stringify(run)).toBe(before);
    expect(after).not.toBe(run);

    let fan = startRun(fanOut(), "manual", at, "r");
    fan = started(fan, "fetch");
    const snap = JSON.stringify(fan);
    const expanded = finished(fan, "fetch", { tickets: TICKETS });
    expect(JSON.stringify(fan)).toBe(snap);
    expect(expanded.nodes.length).toBeGreaterThan(fan.nodes.length);
  });

  it("snapshots the definition at startRun", () => {
    const w = devReview("mono");
    const run = startRun(w, "manual", at, "r");
    (w.nodes[1] as { prompt: string }).prompt = "changed";
    w.nodes.pop();
    expect(run.definition).not.toBe(w);
    expect((run.definition.nodes[1] as { prompt: string }).prompt).toContain("Implement round");
    expect(nextActions(run)[0]).toMatchObject({ nodeId: "dev" });
  });

  describe("failed branches do not loop", () => {
    /** The probe: lint fails while review runs; merge was skipped by the failure. */
    const probe = () =>
      def(
        [trigger, session("dev"), session("lint"), session("review"), session("merge"), report],
        [
          { from: "trigger", to: "dev" },
          { from: "trigger", to: "lint" },
          { from: "dev", to: "review" },
          { from: "review", to: "merge" },
          { from: "lint", to: "merge" },
          { from: "merge", to: "report" },
          { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
        ],
      );

    it("exits instead of asking when the branch already failed", () => {
      let run = startRun(probe(), "manual", at, "r");
      run = complete(run, "dev", {});
      run = started(run, "lint");
      run = started(run, "review");
      run = failed(run, "lint", "boom");
      expect(statusOf(run, "merge")).toBe("skipped");
      run = finished(run, "review", {});
      expect(nodeOf(run, "review").decision).toEqual({ value: "exit", reason: "branch failed" });
      expect(statusOf(run, "merge")).toBe("skipped");
      expect(nodeOf(run, "dev").round).toBe(1);
      expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    });

    it("turns a loop answer that arrives after a failure into an exit", () => {
      let run = startRun(probe(), "manual", at, "r");
      run = complete(run, "dev", {});
      run = started(run, "lint");
      run = complete(run, "review", {});
      expect(nextActions(run)).toEqual([expect.objectContaining({ purpose: "loop" })]);
      run = started(run, "review"); // question in flight
      run = failed(run, "lint", "boom");
      run = decided(run, "review", "loop");
      expect(statusOf(run, "review")).toBe("done");
      expect(nodeOf(run, "review").decision).toEqual({ value: "exit", reason: "branch failed" });
      expect(statusOf(run, "dev")).toBe("done");
      expect(statusOf(run, "merge")).toBe("skipped");
      expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    });

    it("a failed For each branch does not stop another branch from looping", () => {
      const w = fanOut({
        maxParallel: 2,
        children: [
          session("dev", { parent: "each" }),
          session("review", { parent: "each" }),
        ],
        edges: [
          { from: "dev", to: "review" },
          { from: "review", to: "dev", loop: { maxRounds: 3, decide: "mono" } },
        ],
      });
      let run = complete(startRun(w, "manual", at, "r"), "fetch", { tickets: [1, 2] });
      run = started(run, "dev", "each#0");
      run = failed(run, "dev", "boom", "each#0");
      run = complete(run, "dev", {}, "each#1");
      run = complete(run, "review", {}, "each#1");
      expect(nextActions(run)).toEqual([
        expect.objectContaining({ kind: "askMono", purpose: "loop", branch: "each#1" }),
      ]);
      run = started(run, "review", "each#1");
      run = decided(run, "review", "loop", "each#1");
      expect(nodeOf(run, "dev", "each#1").round).toBe(2);
      expect(statusOf(run, "dev", "each#1")).toBe("pending");
      expect(statusOf(run, "review", "each#0")).toBe("skipped");
    });
  });

  describe("a loop around a For each keeps the earlier rounds", () => {
    /** trigger -> fetch -> each(dev) -> review, review loops back to each, review -> report. */
    const eachLoop = (maxRounds = 3, maxParallel = 2) =>
      def(
        [
          trigger,
          session("fetch", { output: { tickets: { type: "list" } } }),
          { id: "each", type: "forEach", in: "{{fetch.tickets}}", as: "item", maxParallel },
          session("dev", { parent: "each", prompt: "Implement {{item.title}}", output: { pr_url: { type: "string" } } }),
          session("review", { output: { verdict: { type: "string" } } }),
          report,
        ],
        [
          { from: "trigger", to: "fetch" },
          { from: "fetch", to: "each" },
          { from: "each", to: "review" },
          { from: "review", to: "report" },
          { from: "review", to: "each", loop: { maxRounds, decide: "mono" } },
        ],
      );

    /** Round 1 where `each#0` fails and `each#1` passes, then Mono sends the loop back. */
    const afterRoundOne = () => {
      let run = complete(startRun(eachLoop(), "manual", at, "r"), "fetch", { tickets: [{ title: "A" }, { title: "B" }] });
      run = started(run, "dev", "each#0", "s-0");
      run = failed(run, "dev", "boom", "each#0");
      run = started(run, "dev", "each#1", "s-1");
      run = finished(run, "dev", { pr_url: "u1" }, "each#1");
      run = complete(run, "review", { verdict: "changes" });
      run = started(run, "review"); // the loop question went out
      return decided(run, "review", "loop");
    };

    it("keeps no output on the For each's own round record", () => {
      const run = afterRoundOne();
      const [record] = nodeOf(run, "each").history ?? [];
      expect(record).toMatchObject({ round: 1, status: "done" });
      expect(record).not.toHaveProperty("output");
    });

    it("archives the branches of the finished round instead of deleting them", () => {
      const run = afterRoundOne();
      expect(statusOf(run, "dev", "each#0@r1")).toBe("failed");
      expect(nodeOf(run, "dev", "each#0@r1").sessionId).toBe("s-0");
      expect(nodeOf(run, "dev", "each#0@r1").error).toBe("boom");
      expect(statusOf(run, "dev", "each#1@r1")).toBe("done");
      expect(nodeOf(run, "dev", "each#1@r1").output).toEqual({ pr_url: "u1" });
      // The live branches start over, and the item list of round 1 is still known.
      expect(statusOf(run, "dev", "each#0")).toBe("pending");
      expect(statusOf(run, "dev", "each#1")).toBe("pending");
      expect(run.items["each@r1"]).toEqual([{ title: "A" }, { title: "B" }]);
    });

    it("never emits or admits an archived branch", () => {
      const run = afterRoundOne();
      const actions = nextActions(run);
      expect(actions.length).toBeGreaterThan(0);
      expect(actions.every((a) => a.kind === "postReport" || !a.branch.includes("@"))).toBe(true);
      expect(actions.map((a) => a.kind === "startSession" && [a.nodeId, a.branch, a.round])).toEqual([
        ["dev", "each#0", 1],
        ["dev", "each#1", 1],
      ]);
      // Events aimed at an archived branch change nothing.
      expect(applyEvent(run, { kind: "nodeStarted", nodeId: "dev", branch: "each#0@r1", at })).toBe(run);
    });

    it("ends doneWithFailures when round 1 failed and round 2 passed", () => {
      let run = afterRoundOne();
      run = started(run, "dev", "each#0", "s-0b");
      run = finished(run, "dev", { pr_url: "u0" }, "each#0");
      run = complete(run, "dev", { pr_url: "u1b" }, "each#1");
      expect(statusOf(run, "each")).toBe("done");
      run = complete(run, "review", { verdict: "approve" });
      run = started(run, "review");
      run = decided(run, "review", "exit");
      expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
      run = applyEvent(run, { kind: "reportPosted", at });
      expect(run.status).toBe("doneWithFailures");
      expect(statusOf(run, "dev", "each#0@r1")).toBe("failed");
    });

    it("keeps every earlier round when the loop runs again", () => {
      let run = afterRoundOne();
      run = complete(run, "dev", { pr_url: "a" }, "each#0");
      run = complete(run, "dev", { pr_url: "b" }, "each#1");
      run = complete(run, "review", { verdict: "changes" });
      run = started(run, "review");
      run = decided(run, "review", "loop");
      expect(nodeOf(run, "dev", "each#0").round).toBe(1);
      expect(nodeOf(run, "review").round).toBe(3);
      for (const key of ["each#0@r1", "each#1@r1", "each#0@r2", "each#1@r2"]) {
        expect(statusOf(run, "dev", key)).toBeDefined();
      }
      expect(statusOf(run, "dev", "each#0@r1")).toBe("failed");
      expect(statusOf(run, "dev", "each#0@r2")).toBe("done");
      expect(run.items["each@r2"]).toHaveLength(2);
      expect(nextActions(run).every((a) => a.kind === "postReport" || !a.branch.includes("@"))).toBe(true);
    });

    it("does not count archived branches against maxParallel", () => {
      let run = complete(startRun(eachLoop(3, 1), "manual", at, "r"), "fetch", { tickets: [1, 2] });
      run = complete(run, "dev", {}, "each#0");
      run = complete(run, "dev", {}, "each#1");
      run = complete(run, "review", {});
      run = started(run, "review");
      run = decided(run, "review", "loop");
      expect(nextActions(run).map((a) => a.kind === "startSession" && [a.nodeId, a.branch])).toEqual([["dev", "each#0"]]);
    });

    it("an archived failure does not stop the outer loop from running another round", () => {
      const run = afterRoundOne();
      expect(nodeOf(run, "review").round).toBe(2);
      expect(nodeOf(run, "review").decision).toBeUndefined();
    });
  });

  it("if true/false skips the other side", () => {
    const w = def(
      [
        trigger,
        session("a", { output: { ok: { type: "boolean" } } }),
        { id: "gate", type: "if", condition: "{{a.ok}} == true" },
        session("yes"),
        session("no"),
        session("after"),
        report,
      ],
      [
        { from: "trigger", to: "a" },
        { from: "a", to: "gate" },
        { from: "gate", to: "yes", handle: "true" },
        { from: "gate", to: "no", handle: "false" },
        { from: "no", to: "after" },
        { from: "yes", to: "report" },
        { from: "after", to: "report" },
      ],
    );
    let t = complete(startRun(w, "manual", at, "r"), "a", { ok: true });
    expect(statusOf(t, "gate")).toBe("done");
    expect(nodeOf(t, "gate").decision).toEqual({ value: "true" });
    expect(statusOf(t, "no")).toBe("skipped");
    expect(statusOf(t, "after")).toBe("skipped"); // transitive
    expect(nextActions(t).map((x) => x.kind === "startSession" && x.nodeId)).toEqual(["yes"]);
    t = complete(t, "yes", {});
    expect(nextActions(t)).toEqual([{ kind: "postReport" }]);

    const f = complete(startRun(w, "manual", at, "r"), "a", { ok: false });
    expect(nodeOf(f, "gate").decision).toEqual({ value: "false" });
    expect(statusOf(f, "yes")).toBe("skipped");
    expect(nextActions(f).map((x) => x.kind === "startSession" && x.nodeId)).toEqual(["no"]);
  });

  it("if decided by the Mono asks true/false", () => {
    const w = def(
      [
        trigger,
        { id: "gate", type: "if", decide: "mono", question: "Is {{inputs.project}} fine?" },
        session("yes"),
        report,
      ],
      [
        { from: "trigger", to: "gate" },
        { from: "gate", to: "yes", handle: "true" },
        { from: "yes", to: "report" },
        { from: "gate", to: "report", handle: "false" },
      ],
    );
    let run = startRun(w, "manual", at, "r");
    expect(nextActions(run)).toEqual([
      expect.objectContaining({
        kind: "askMono",
        purpose: "if",
        question: "Is /work/repo fine?",
        answers: ["true", "false"],
      }),
    ]);
    run = started(run, "gate");
    expect(nextActions(run)).toEqual([]);
    run = decided(run, "gate", "false");
    expect(statusOf(run, "yes")).toBe("skipped");
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
  });

  it("askMono node records answer and reason", () => {
    const w = def(
      [trigger, { id: "pick", type: "askMono", question: "Which?", answers: ["a", "b"] }, session("s", { prompt: "Got {{pick.answer}}" }), report],
      [
        { from: "trigger", to: "pick" },
        { from: "pick", to: "s" },
        { from: "s", to: "report" },
      ],
    );
    let run = startRun(w, "manual", at, "r");
    expect(nextActions(run)).toEqual([
      expect.objectContaining({ kind: "askMono", purpose: "askMono", question: "Which?", answers: ["a", "b"] }),
    ]);
    run = started(run, "pick");
    run = decided(run, "pick", "b", "", "because");
    expect(nodeOf(run, "pick").output).toEqual({ answer: "b", reason: "because" });
    expect(nextActions(run)).toEqual([expect.objectContaining({ prompt: "Got b" })]);
  });

  it("failure in one branch does not stop another; status doneWithFailures", () => {
    const w = fanOut({
      maxParallel: 3,
      children: [
        session("dev", { parent: "each", output: { pr_url: { type: "string" } } }),
        session("review", { parent: "each" }),
      ],
      edges: [{ from: "dev", to: "review" }],
    });
    let run = complete(startRun(w, "manual", at, "r"), "fetch", { tickets: [1, 2] });
    run = started(run, "dev", "each#0");
    run = started(run, "dev", "each#1");
    run = failed(run, "dev", "boom", "each#0");
    expect(statusOf(run, "review", "each#0")).toBe("skipped");
    expect(nextActions(run)).toEqual([]);
    run = finished(run, "dev", { pr_url: "u" }, "each#1");
    expect(nextActions(run)).toEqual([expect.objectContaining({ nodeId: "review", branch: "each#1" })]);
    run = complete(run, "review", {}, "each#1");
    expect(nodeOf(run, "each").output).toEqual({ results: [null, {}] });
    run = applyEvent(run, { kind: "reportPosted", at: 9 });
    expect(run.status).toBe("doneWithFailures");
  });

  it("run is failed when no session or command succeeded", () => {
    let run = startRun(
      def([trigger, session("a"), report], [
        { from: "trigger", to: "a" },
        { from: "a", to: "report" },
      ]),
      "manual",
      at,
      "r",
    );
    run = failed(started(run, "a"), "a", "nope");
    expect(nextActions(run)).toEqual([{ kind: "postReport" }]);
    run = applyEvent(run, { kind: "reportPosted", at: 5 });
    expect(run.status).toBe("failed");
  });

  it("a run without a report node ends as soon as nothing is left", () => {
    let run = startRun(
      def([trigger, session("a")], [{ from: "trigger", to: "a" }]),
      "manual",
      at,
      "r",
    );
    run = complete(run, "a", {}, "", 7);
    expect(run.status).toBe("done");
    expect(run.endedAt).toBe(7);
    expect(nextActions(run)).toEqual([]);
  });

  it("markInterrupted keeps done nodes and resets running to pending", () => {
    let run = startRun(fanOut(), "manual", at, "r");
    run = complete(run, "fetch", { tickets: TICKETS });
    run = started(run, "dev", "each#0", "s-0");
    run = started(run, "dev", "each#1", "s-1");
    run = finished(run, "dev", { pr_url: "u1" }, "each#1");
    const hurt = markInterrupted(run);
    expect(hurt.status).toBe("interrupted");
    expect(statusOf(hurt, "fetch")).toBe("done");
    expect(statusOf(hurt, "dev", "each#1")).toBe("done");
    expect(statusOf(hurt, "dev", "each#0")).toBe("pending");
    expect(nodeOf(hurt, "dev", "each#0").sessionId).toBeUndefined();
    expect(statusOf(hurt, "each")).toBe("running"); // the group keeps its expansion
    expect(hurt.items.each).toEqual(TICKETS);
    expect(nextActions(hurt)).toEqual([]);
    expect(applyEvent(hurt, { kind: "nodeFinished", nodeId: "dev", branch: "each#0", output: {}, at })).toBe(hurt);
  });

  describe("resuming a node that was mid-session", () => {
    const startAction = (run: WorkflowRun) => {
      const [action] = sessions(nextActions(run));
      if (action?.kind !== "startSession") throw new Error("no startSession");
      return action;
    };

    it("reuses the loop session of round 2 or later", () => {
      let run = startRun(devReview("mono"), "manual", at, "r");
      run = complete(started(run, "dev", "", "s-dev"), "dev", { pr_url: "p1" });
      run = complete(run, "review", { verdict: "no" });
      run = started(run, "review");
      run = decided(run, "review", "loop");
      run = started(run, "dev", "", "s-dev-2"); // dev is running round 2
      const resumed = resumeRun(markInterrupted(run));
      expect(nodeOf(resumed, "dev").sessionId).toBeUndefined();
      expect(startAction(resumed)).toMatchObject({ nodeId: "dev", round: 2, reuseSessionId: "s-dev-2" });
      // Interrupted again before it restarted: the id is not lost.
      expect(startAction(resumeRun(markInterrupted(resumed)))).toMatchObject({ reuseSessionId: "s-dev-2" });
    });

    it("does not reuse when the node opted out of reuse", () => {
      let run = startRun(devReview("mono", 3, false), "manual", at, "r");
      run = complete(started(run, "dev", "", "s-dev"), "dev", {});
      run = complete(run, "review", {});
      run = started(run, "review");
      run = decided(run, "review", "loop");
      run = started(run, "dev", "", "s-dev-2");
      expect(startAction(resumeRun(markInterrupted(run))).reuseSessionId).toBeUndefined();
    });

    it("re-runs a round 1 session fresh", () => {
      let run = startRun(devReview("mono"), "manual", at, "r");
      run = started(run, "dev", "", "s-dev");
      const action = startAction(resumeRun(markInterrupted(run)));
      expect(action.round).toBe(1);
      expect(action.reuseSessionId).toBeUndefined();
    });
  });

  it("resumeRun never re-emits a done node", () => {
    let run = startRun(fanOut(), "manual", at, "r");
    run = complete(run, "fetch", { tickets: TICKETS });
    run = started(run, "dev", "each#0");
    run = started(run, "dev", "each#1");
    run = finished(run, "dev", { pr_url: "u1" }, "each#1");
    const resumed = resumeRun(markInterrupted(run));
    expect(resumed.status).toBe("running");
    const actions = nextActions(resumed);
    expect(actions.map((a) => a.kind === "startSession" && [a.nodeId, a.branch])).toEqual([
      ["dev", "each#0"],
      ["dev", "each#2"],
    ]);
    expect(actions.some((a) => a.kind === "startSession" && a.nodeId === "fetch")).toBe(false);
  });

  it("resumeRun after a loop question was lost asks again without rerunning the node", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p" });
    run = complete(run, "review", { verdict: "no" });
    run = started(run, "review"); // question sent
    const resumed = resumeRun(markInterrupted(run));
    expect(statusOf(resumed, "review")).toBe("done");
    expect(nextActions(resumed)).toEqual([expect.objectContaining({ kind: "askMono", purpose: "loop" })]);
  });

  it("stopped event ends run as stopped and emits nothing", () => {
    let run = startRun(fanOut(), "manual", at, "r");
    run = started(run, "fetch");
    run = applyEvent(run, { kind: "stopped", at: 50 });
    expect(run.status).toBe("stopped");
    expect(run.endedAt).toBe(50);
    expect(statusOf(run, "fetch")).toBe("stopped");
    expect(statusOf(run, "each")).toBe("skipped");
    expect(nextActions(run)).toEqual([]);
    expect(applyEvent(run, { kind: "nodeFinished", nodeId: "fetch", branch: "", output: {}, at })).toBe(run);
  });

  it("waitingApproval toggles a running node", () => {
    let run = startRun(
      def([trigger, session("a"), report], [
        { from: "trigger", to: "a" },
        { from: "a", to: "report" },
      ]),
      "manual",
      at,
      "r",
    );
    run = started(run, "a");
    run = applyEvent(run, { kind: "waitingApproval", nodeId: "a", branch: "", waiting: true });
    expect(statusOf(run, "a")).toBe("waitingApproval");
    expect(nextActions(run)).toEqual([]);
    run = applyEvent(run, { kind: "waitingApproval", nodeId: "a", branch: "", waiting: false });
    expect(statusOf(run, "a")).toBe("running");
  });

  it("events are immutable updates", () => {
    const run = startRun(
      def([trigger, session("a"), report], [
        { from: "trigger", to: "a" },
        { from: "a", to: "report" },
      ]),
      "manual",
      at,
      "r",
    );
    const snapshot = JSON.stringify(run);
    const next = started(run, "a", "", "s");
    expect(JSON.stringify(run)).toBe(snapshot);
    expect(next).not.toBe(run);
  });

  it("runScope exposes inputs, run, the loop variable and branch outputs", () => {
    let run = startRun(fanOut(), "manual", 77, "run-x");
    run = complete(run, "fetch", { tickets: TICKETS });
    run = complete(run, "dev", { pr_url: "u0" }, "each#0");
    const scope = runScope(run, "each#0", "dev") as Record<string, any>;
    expect(scope.inputs).toEqual({ project: "/work/repo" });
    expect(scope.run).toEqual({ id: "run-x", startedAt: 77, round: 1 });
    expect(scope.item).toEqual({ title: "A" });
    expect(scope.fetch).toEqual({ tickets: TICKETS });
    expect(scope.dev).toEqual({ pr_url: "u0" });
    const other = runScope(run, "each#1") as Record<string, any>;
    expect(other.item).toEqual({ title: "B" });
    expect(other.dev).toBeUndefined();
    expect((runScope(run, "") as Record<string, any>).item).toBeUndefined();
  });

  it("run.round is the current round of the node inside a loop", () => {
    let run = startRun(devReview("mono"), "manual", at, "r");
    run = complete(run, "dev", { pr_url: "p" });
    run = complete(run, "review", { verdict: "no" });
    run = decided(run, "review", "loop");
    expect((runScope(run, "", "dev") as Record<string, any>).run.round).toBe(2);
  });

  it("does not run nodes the trigger cannot reach", () => {
    const run = startRun(
      def([trigger, session("a"), session("orphan"), report], [
        { from: "trigger", to: "a" },
        { from: "a", to: "report" },
      ]),
      "manual",
      at,
      "r",
    );
    expect(statusOf(run, "orphan")).toBe("skipped");
    expect(nextActions(run).map((a) => a.kind === "startSession" && a.nodeId)).toEqual(["a"]);
  });

  it("ignores events for unknown nodes", () => {
    const run = startRun(def([trigger, report], [{ from: "trigger", to: "report" }]), "manual", at, "r");
    const bogus: EngineEvent = { kind: "nodeFinished", nodeId: "ghost", branch: "", output: {}, at };
    expect(applyEvent(run, bogus)).toBe(run);
  });
});

describe("loopedNodeIds", () => {
  const loop = { maxRounds: 3, decide: "mono" } as const;

  it("names every node from the loop's target to its source, and no other", () => {
    const w = def(
      [trigger, session("prep"), session("dev"), session("lint"), session("review"), session("ship"), report],
      [
        { from: "trigger", to: "prep" },
        { from: "prep", to: "dev" },
        { from: "dev", to: "lint" },
        { from: "lint", to: "review" },
        { from: "review", to: "ship" },
        { from: "ship", to: "report" },
        { from: "review", to: "dev", loop },
      ],
    );
    expect([...loopedNodeIds(w)].sort()).toEqual(["dev", "lint", "review"]);
  });

  it("is empty without a loop", () => {
    const w = def([trigger, session("a"), report], [
      { from: "trigger", to: "a" },
      { from: "a", to: "report" },
    ]);
    expect(loopedNodeIds(w).size).toBe(0);
  });

  it("stays within one For each", () => {
    const w = def(
      [
        trigger,
        session("fetch", { output: { tickets: { type: "list" } } }),
        { id: "each", type: "forEach", in: "{{fetch.tickets}}", as: "item", maxParallel: 2 },
        session("dev", { parent: "each" }),
        session("review", { parent: "each" }),
        report,
      ],
      [
        { from: "trigger", to: "fetch" },
        { from: "fetch", to: "each" },
        { from: "dev", to: "review" },
        { from: "each", to: "report" },
        { from: "review", to: "dev", loop },
      ],
    );
    expect([...loopedNodeIds(w)].sort()).toEqual(["dev", "review"]);
  });
});
