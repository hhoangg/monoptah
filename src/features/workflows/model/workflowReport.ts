// What a finished run tells the Mono: the report the app composes itself, plus the prompt and reply
// parsing for the questions the engine puts to the Mono (Ask Mono nodes, Mono-decided Ifs and loops).
import type { NodeId, NodeRun, OutputField, RunStatus, SessionNode, WorkflowRun } from "./workflow";
import { BRANCH_FAILED_REASON, MAX_ROUNDS_REASON, type EngineAction } from "./workflowEngine";
import { OUTPUT_FENCE, parseWorkflowOutput } from "./workflowOutput";
import { renderTemplate } from "./workflowTemplate";

const RUN_STATUS_LABEL: Record<RunStatus, string> = {
  running: "running",
  done: "completed",
  doneWithFailures: "completed with failures",
  failed: "failed",
  stopped: "stopped",
  interrupted: "interrupted",
};

/** A string longer than this is shown as a quoted block instead of inline. */
const INLINE_STRING_MAX = 200;
const ACTION_TYPES = new Set(["session", "command", "askMono", "if"]);

type BranchResult = {
  branch: string;
  name: string;
  status: string;
  rounds: number;
  maxRoundsReached: boolean;
  /** One entry per failed node. */
  errors: string[];
  /** Nodes whose loop ended because the branch had failed. */
  loopsCutShort: string[];
  /** Output of the branch's last session node, whole (templates may use any field). */
  output: Record<string, unknown>;
  /** The fields of that output the report shows. */
  shown: Array<[string, unknown]>;
};

const oneLine = (text: string) => text.replace(/\s*[\r\n]+\s*/g, " ").trim();

/** A fence longer than any backtick run in `body`, so the body cannot close it early. */
function fenceFor(body: string): string {
  const longest = Math.max(0, ...(body.match(/`+/g) ?? []).map((run) => run.length));
  return "`".repeat(Math.max(3, longest + 1));
}

function nodeLabel(run: WorkflowRun, nodeId: NodeId): string {
  return run.definition.nodes.find((n) => n.id === nodeId)?.title ?? nodeId;
}

/** "title" or "name" of a For each item when it is an object that has one; else "#<n>". */
function itemLabel(item: unknown, index: number): string {
  if (item !== null && typeof item === "object" && !Array.isArray(item)) {
    const record = item as Record<string, unknown>;
    for (const key of ["title", "name"]) {
      const value = record[key];
      if (typeof value === "string" && value.trim()) return oneLine(value);
    }
  }
  return `#${index + 1}`;
}

function branchStatus(runs: NodeRun[]): string {
  if (runs.length === 0) return "pending";
  const has = (...statuses: NodeRun["status"][]) => runs.some((r) => statuses.includes(r.status));
  if (has("failed")) return "failed";
  if (has("stopped")) return "stopped";
  if (has("waitingApproval")) return "waiting for approval";
  if (has("running")) return "running";
  if (has("pending")) return "pending";
  return has("done") ? "done" : "skipped";
}

/** The session node run that finished last in the branch and produced output. */
function lastSessionRun(run: WorkflowRun, runs: NodeRun[]): NodeRun | undefined {
  let best: NodeRun | undefined;
  for (const r of runs) {
    const node = run.definition.nodes.find((n) => n.id === r.nodeId);
    if (node?.type !== "session" || r.status !== "done" || r.output === undefined) continue;
    if (!best || (r.endedAt ?? r.startedAt ?? 0) >= (best.endedAt ?? best.startedAt ?? 0)) best = r;
  }
  return best;
}

/** Whether the report shows a field: `inReport` when declared, else on for strings and off for the rest. */
export function fieldInReport(field: OutputField): boolean {
  return field.inReport ?? field.type === "string";
}

/** Fields the report shows. */
function reportedFields(node: SessionNode, output: Record<string, unknown>): Array<[string, unknown]> {
  const shown: Array<[string, unknown]> = [];
  for (const [name, field] of Object.entries(node.output) as Array<[string, OutputField]>) {
    if (!fieldInReport(field)) continue;
    if (!Object.prototype.hasOwnProperty.call(output, name)) continue;
    shown.push([name, output[name]]);
  }
  return shown;
}

/** Reported fields of every finished session node in the branch, as `<node>.<field>`, in finish order. */
function allSessionFields(run: WorkflowRun, runs: NodeRun[]): Array<[string, unknown]> {
  const finished = runs
    .filter((r) => r.status === "done" && r.output !== undefined)
    .sort((a, b) => (a.endedAt ?? a.startedAt ?? 0) - (b.endedAt ?? b.startedAt ?? 0));
  return finished.flatMap((r) => {
    const node = run.definition.nodes.find((n) => n.id === r.nodeId);
    if (node?.type !== "session") return [];
    return reportedFields(node, r.output ?? {}).map(([name, value]): [string, unknown] => [`${r.nodeId}.${name}`, value]);
  });
}

function resultOf(run: WorkflowRun, branch: string, name: string, allSessions = false): BranchResult {
  const loopNodes = new Set(run.definition.edges.filter((e) => e.loop).map((e) => e.from));
  const runs = run.nodes.filter((r) => r.branch === branch && ACTION_TYPES.has(nodeType(run, r.nodeId)));
  const last = lastSessionRun(run, runs);
  const lastNode = last && run.definition.nodes.find((n) => n.id === last.nodeId);
  const output = last?.output ?? {};
  return {
    branch,
    name,
    status: branchStatus(runs),
    rounds: Math.max(0, ...runs.filter((r) => loopNodes.has(r.nodeId)).map((r) => r.round)),
    maxRoundsReached: runs.some((r) => r.decision?.reason === MAX_ROUNDS_REASON),
    errors: runs
      .filter((r) => r.status === "failed")
      .map((r) => `${nodeLabel(run, r.nodeId)}: ${oneLine(r.error ?? "failed")}`),
    loopsCutShort: runs
      .filter((r) => r.decision?.reason === BRANCH_FAILED_REASON)
      .map((r) => nodeLabel(run, r.nodeId)),
    output,
    shown: allSessions
      ? allSessionFields(run, runs)
      : lastNode?.type === "session"
        ? reportedFields(lastNode, output)
        : [],
  };
}

function nodeType(run: WorkflowRun, nodeId: NodeId): string {
  return run.definition.nodes.find((n) => n.id === nodeId)?.type ?? "";
}

/** Branch results in report order, plus the For each nodes that had nothing to run. */
function collect(run: WorkflowRun): { results: BranchResult[]; empty: string[]; groups: number } {
  const groups = run.definition.nodes.filter((n) => n.type === "forEach");
  const results: BranchResult[] = [];
  const empty: string[] = [];
  for (const group of groups) {
    const items = run.items[group.id];
    if (items === undefined) continue;
    if (items.length === 0) empty.push(nodeLabel(run, group.id));
    items.forEach((item, index) =>
      results.push(resultOf(run, `${group.id}#${index}`, itemLabel(item, index))),
    );
  }
  // Work outside any For each. With no For each at all this is the whole run.
  // Next to a For each it also lists the outputs of steps before or after it (e.g. a summary).
  const top = resultOf(run, "", "Run", groups.length > 0);
  if (groups.length === 0 || top.status === "failed" || top.errors.length > 0 || top.shown.length > 0) {
    results.unshift(top);
  }
  return { results, empty, groups: groups.length };
}

function renderValue(name: string, value: unknown): string[] {
  if (typeof value === "string") {
    if (value.length <= INLINE_STRING_MAX) return [`- ${name}: ${oneLine(value)}`];
    return [`- ${name}:`, ...value.split(/\r?\n/).map((line) => `  > ${line}`)];
  }
  if (value === null || typeof value !== "object") return [`- ${name}: ${String(value)}`];
  const json = JSON.stringify(value, null, 2);
  const fence = fenceFor(json);
  return [`- ${name}:`, `  ${fence}json`, ...json.split("\n").map((line) => `  ${line}`), `  ${fence}`];
}

function renderBranch(result: BranchResult): string {
  const lines = [`### ${result.name}`, `- Status: ${result.status}`];
  if (result.rounds > 0) lines.push(`- Loop rounds used: ${result.rounds}`);
  if (result.maxRoundsReached) lines.push("- Max rounds reached");
  for (const node of result.loopsCutShort) lines.push(`- Loop after ${node} ended: branch failed`);
  for (const error of result.errors) lines.push(`- Error in ${error}`);
  for (const [name, value] of result.shown) lines.push(...renderValue(name, value));
  return lines.join("\n");
}

/** The report the app posts to the Mono when a run ends, as Markdown. */
export function composeReport(run: WorkflowRun): { title: string; text: string } {
  const workflowName = run.definition.name;
  const statusLabel = RUN_STATUS_LABEL[run.status];
  const title = `${workflowName} · ${statusLabel}`;
  const { results, empty, groups } = collect(run);

  const reportNode = run.definition.nodes.find((n) => n.type === "report");
  const template = reportNode?.type === "report" ? reportNode.template : undefined;
  let body: string;
  if (template !== undefined && template.trim() !== "") {
    body = renderTemplate(template, {
      run: {
        id: run.id,
        name: workflowName,
        status: run.status,
        statusLabel,
        startedAt: run.startedAt,
        endedAt: run.endedAt,
      },
      results: results.map((r) => ({
        branch: r.branch,
        name: r.name,
        status: r.status,
        rounds: r.rounds,
        maxRoundsReached: r.maxRoundsReached,
        errors: r.errors,
        output: r.output,
      })),
    });
  } else {
    const parts: string[] = [];
    if (run.error) parts.push(`Run error: ${oneLine(run.error)}`);
    for (const label of empty) parts.push(groups > 1 ? `No items in ${label}` : "No items");
    for (const result of results) parts.push(renderBranch(result));
    body = parts.join("\n\n");
  }
  return { title, text: `## ${title}\n\n${body}`.trimEnd() };
}

/** The request the Mono receives for an Ask Mono node, or a Mono-decided If or loop. */
export function askMonoPrompt(
  action: Extract<EngineAction, { kind: "askMono" }>,
  workflowName: string,
): string {
  const context = JSON.stringify(action.context, null, 2);
  const fence = fenceFor(context);
  return [
    `You are being consulted by a running workflow, "${oneLine(workflowName)}", at step "${action.nodeId}".`,
    "This is a single decision request. Do not start any work or use any tools; just decide.",
    "",
    "Question:",
    action.question,
    "",
    "Relevant outputs so far:",
    `${fence}json`,
    context,
    fence,
    "",
    `Allowed answers: ${action.answers.map((a) => JSON.stringify(a)).join(", ")}`,
    "",
    `Reply with a short explanation if you like, and end with exactly one fenced code block whose info string is \`${OUTPUT_FENCE}\`:`,
    "```" + OUTPUT_FENCE,
    '{"answer": <one of the allowed answers>, "reason": "..."}',
    "```",
  ].join("\n");
}

/** The Mono's decision from its reply: `answer` must be one of `answers`; `reason` is optional. */
export function parseMonoDecision(
  reply: string,
  answers: string[],
): { ok: true; value: string; reason?: string } | { ok: false; error: string } {
  const parsed = parseWorkflowOutput(reply, { answer: { type: "string" } });
  if (!parsed.ok) return { ok: false, error: parsed.error };
  const answer = parsed.value.answer as string;
  if (!answers.includes(answer)) {
    return {
      ok: false,
      error: `Answer ${JSON.stringify(answer)} is not allowed; choose one of: ${answers.map((a) => JSON.stringify(a)).join(", ")}.`,
    };
  }
  const reason = parsed.value.reason;
  return typeof reason === "string" && reason !== "" ? { ok: true, value: answer, reason } : { ok: true, value: answer };
}
