import type { HabitSchedule } from "../../monos/model/monoHabits";

/**
 * A workflow is a graph of nodes a Mono runs for the user: a trigger, actions (sessions,
 * shell commands, questions for the Mono), flow control (If, For each) and one Report node.
 * This file holds only the definition. Definitions never contain absolute paths or session
 * ids, so a workflow can be exported and shared.
 */

/** "wf-" followed by 8 lowercase base36 characters. */
export type WorkflowId = string;
/** Matches `/^[a-z][a-z0-9_]{0,39}$/`, so it is safe inside a `{{nodeId.field}}` path. */
export type NodeId = string;
export type OutputFieldType = "string" | "number" | "boolean" | "list" | "object";
export type OutputField = {
  type: OutputFieldType;
  inReport?: boolean;
  description?: string;
};
export type Checkout = "root" | "newWorktree" | { worktreeOf: NodeId };
export type WorkflowInput = {
  type: "project" | "string" | "number" | "boolean";
  description?: string;
};

type Base = {
  id: NodeId;
  title?: string;
  /** The For each node this node runs inside. */
  parent?: NodeId;
  /** Board position, kept so an exported file opens with the same layout. */
  ui?: { x: number; y: number };
};

export type ScheduleNode = Base & { type: "schedule"; schedule: HabitSchedule };
export type ManualNode = Base & { type: "manual" };
export type SessionNode = Base & {
  type: "session";
  /** Always `{{inputs.<name>}}` naming a `project` input. */
  project: string;
  prompt: string;
  harness?: string;
  model?: string;
  effort?: string;
  checkout: Checkout;
  /** Keep one session across loop rounds. Default true. */
  reuseSession?: boolean;
  nestUnder?: NodeId;
  output: Record<string, OutputField>;
  /** Default 120. */
  timeoutMinutes?: number;
};
export type CommandNode = Base & {
  type: "command";
  project: string;
  command: string;
  cwd?: "root" | { worktreeOf: NodeId };
  /** Default 10. */
  timeoutMinutes?: number;
};
export type AskMonoNode = Base & {
  type: "askMono";
  question: string;
  answers: string[];
};
/** Either `condition` or `decide: "mono"` (with an optional `question`). */
export type IfNode = Base & {
  type: "if";
  condition?: string;
  decide?: "mono";
  question?: string;
};
export type ForEachNode = Base & {
  type: "forEach";
  /** A list expression such as `{{fetch.tickets}}`. */
  in: string;
  /** Name of the loop variable. Default "item". */
  as?: string;
  /** 1 to 10. Default 3. */
  maxParallel?: number;
};
export type ReportNode = Base & { type: "report"; template?: string };
export type WorkflowNode =
  | ScheduleNode
  | ManualNode
  | SessionNode
  | CommandNode
  | AskMonoNode
  | IfNode
  | ForEachNode
  | ReportNode;
export type NodeType = WorkflowNode["type"];

/** A back edge: after the source node, the decider may send the branch back to `to`. */
export type LoopSpec = {
  /** 1 to 10. */
  maxRounds: number;
  /** "mono", or a condition such as `{{review.verdict}} != "approve"`. */
  decide: "mono" | string;
};
export type WorkflowEdge = {
  from: NodeId;
  to: NodeId;
  /** Only on the out edges of an If node. */
  handle?: "true" | "false";
  loop?: LoopSpec;
};

export type Workflow = {
  id: WorkflowId;
  name: string;
  description?: string;
  inputs: Record<string, WorkflowInput>;
  /** Values for the declared inputs. Not exported. */
  inputValues: Record<string, string | number | boolean>;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  enabled: boolean;
  createdAt: number;
  nextRunAt?: number;
  runRequested?: boolean;
  /**
   * Requests made from another window, kept in the file because only the main window drives runs.
   * `stopRequested` asks it to stop the workflow's running run; `resumeRunId` asks it to resume
   * that interrupted run. The driver clears each once it has acted on it.
   */
  stopRequested?: boolean;
  resumeRunId?: string;
  lastRunId?: string;
  importedFrom?: string;
  /**
   * Set on a workflow the Mono created from a file through the CLI. Its commands and prompts
   * came from elsewhere, so only the user, in the panel, may enable or run it; that clears
   * the mark. Never exported.
   */
  needsReview?: boolean;
};

export const WORKFLOWS_MAX = 20;
export const WORKFLOW_RUNS_KEPT = 20;

export const NODE_ID = /^[a-z][a-z0-9_]{0,39}$/;
export const NODE_TYPES: readonly NodeType[] = [
  "schedule",
  "manual",
  "session",
  "command",
  "askMono",
  "if",
  "forEach",
  "report",
];

const BASE36 = "0123456789abcdefghijklmnopqrstuvwxyz";

export function newWorkflowId(): WorkflowId {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  // 256 is not a multiple of 36, so the modulo skews slightly; an id is a label, not a secret.
  return `wf-${Array.from(bytes, (b) => BASE36[b % 36]).join("")}`;
}

export function isTrigger(node: WorkflowNode): node is ScheduleNode | ManualNode {
  return node.type === "schedule" || node.type === "manual";
}

/** The workflow's trigger node, when it has one. */
export function triggerOf(w: Workflow): ScheduleNode | ManualNode | undefined {
  return w.nodes.find(isTrigger);
}

export type NodeStatus =
  | "pending"
  | "running"
  | "waitingApproval"
  | "done"
  | "failed"
  | "skipped"
  | "stopped";
/**
 * "" for top level, "<forEachId>#<index>" inside a group. A loop that goes back through a For
 * each archives the branches of the round that just finished as "<forEachId>#<index>@r<round>";
 * their nodes are settled for good and their items stay under `items["<forEachId>@r<round>"]`.
 */
export type BranchKey = string;

const BRANCH_PATTERN = /^(.+)#(\d+)(?:@r(\d+))?$/;

/** The parts of a For each branch key, or undefined for the top level. `round` is set on archived branches. */
export function parseBranch(
  branch: BranchKey,
): { groupId: NodeId; index: number; round?: number } | undefined {
  const match = BRANCH_PATTERN.exec(branch);
  if (!match) return undefined;
  return {
    groupId: match[1],
    index: Number(match[2]),
    ...(match[3] !== undefined ? { round: Number(match[3]) } : {}),
  };
}

export const isArchivedBranch = (branch: BranchKey): boolean => parseBranch(branch)?.round !== undefined;
export const archivedBranch = (branch: BranchKey, round: number): BranchKey => `${branch}@r${round}`;
/** Where the item list of a For each is kept once a loop round archived it. */
export const archivedItemsKey = (groupId: NodeId, round: number): NodeId => `${groupId}@r${round}`;
export type NodeRun = {
  nodeId: NodeId;
  branch: BranchKey;
  status: NodeStatus;
  round: number;
  sessionId?: string;
  output?: Record<string, unknown>;
  error?: string;
  decision?: { value: string; reason?: string };
  startedAt?: number;
  endedAt?: number;
  reasked?: boolean;
};
export type RunStatus =
  | "running"
  | "done"
  | "doneWithFailures"
  | "failed"
  | "stopped"
  | "interrupted";
export type WorkflowRun = {
  id: string;
  workflowId: WorkflowId;
  /** Snapshot of the definition the run started from. */
  definition: Workflow;
  status: RunStatus;
  startedAt: number;
  endedAt?: number;
  trigger: "schedule" | "manual" | "cli";
  folderId?: string;
  items: Record<NodeId, unknown[]>;
  nodes: NodeRun[];
  reportPostedAt?: number;
  error?: string;
};
