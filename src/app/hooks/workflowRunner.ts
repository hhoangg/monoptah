import type { ControlOutcome } from "../../features/orchestration/model/orchestration";
import type {
  Block,
  MonoSpawnedSession,
  Session,
} from "../../features/sessions/model/session";
import {
  HABIT_APPROVAL_WAIT_MS,
  isRelayed,
} from "../../features/monos/model/monoHabits";
import type {
  NodeId,
  SessionNode,
  Workflow,
  WorkflowRun,
} from "../../features/workflows/model/workflow";
import {
  applyEvent,
  isAsking,
  nextActions,
  runScope,
  type BranchKey,
  type EngineAction,
  type EngineEvent,
} from "../../features/workflows/model/workflowEngine";
import {
  outputReask,
  parseWorkflowOutput,
} from "../../features/workflows/model/workflowOutput";
import {
  askMonoPrompt,
  composeReport,
  parseMonoDecision,
} from "../../features/workflows/model/workflowReport";
import {
  clearWorkflowStop,
  loadWorkflows,
  requestWorkflowStop,
  setWorkflowRunning,
  subscribeWorkflowsRunning,
  updateRuns,
  updateWorkflows,
  workflowStopRequested,
} from "../../features/workflows/model/workflowStore";
import { renderTemplate } from "../../features/workflows/model/workflowTemplate";
import { finalReply } from "./useMonoHabits";
import { LIVE_RUN_REFRESH_MS, liveRunRegistry } from "./workflowLiveRuns";

export type SessionRequest = {
  /** A project path of the Mono. */
  project: string;
  prompt: string;
  harness?: string;
  model?: string;
  effort?: string;
  workspaceMode: "current" | "worktree";
  worktreeCwd?: string;
  parentSessionId?: string;
  folderId?: string;
  folderName?: string;
};

export type CommandResult = {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
};

export type MonoWorkflowHost = {
  sessions(): Session[];
  /** Starts a visible project session owned by the Mono, like sessions.start, without a completion report to the Mono. */
  startSession(
    monoId: string,
    req: SessionRequest,
  ): Promise<{
    sessionId: string;
    folderId?: string;
    settled: Promise<ControlOutcome>;
  }>;
  /** Sends a follow-up turn to an existing session and resolves when it settles. */
  send(sessionId: string, prompt: string): Promise<ControlOutcome>;
  stop(sessionId: string): Promise<void>;
  runCommand(
    cwd: string,
    command: string,
    timeoutMs: number,
  ): Promise<CommandResult>;
  /** Hidden Mono-context turn (habit-style ephemeral session with /operator), returns the final reply. */
  askMono(monoId: string, prompt: string): Promise<string>;
  post(
    monoSessionId: string,
    workflow: Workflow,
    runId: string,
    title: string,
    text: string,
    launches?: MonoSpawnedSession[],
  ): void;
  /**
   * Puts an approval a node's session is waiting on to the user, in the Mono's chat.
   * Relays are keyed by `sessionId`, the session that asked, which `endApprovals` withdraws by.
   */
  askApproval(
    monoSessionId: string,
    workflow: Workflow,
    runId: string,
    sessionId: string,
    block: Block,
  ): void;
  /** Withdraws the unanswered approvals a session put to the user. */
  endApprovals(sessionId: string): void;
  /** Resolves a `{{inputs.x}}` value to one of the Mono's project paths. */
  projectPath(monoId: string, projectInput: string): string | undefined;
  /** The working directory of a node's session when it runs in a worktree. */
  worktreeOf(sessionId: string): string | undefined;
};

const POLL_MS = 2_000;
/** How often a run looks in the workflow file for a stop asked for in another window. */
const STOP_CHECK_MS = 5_000;
const SESSION_TIMEOUT_MINUTES = 120;
const COMMAND_TIMEOUT_MINUTES = 10;
/** How long the Mono has to answer a decision request. */
export const ASK_TIMEOUT_MS = 10 * 60_000;
/** Longest string kept in a node output, so a run record stays small. */
export const OUTPUT_STRING_MAX = 16_000;
const ERROR_MAX = 1_000;
const STOPPED_BY_USER = "Stopped by the user.";

/** Shortens every long string inside a node output, saying so where it cut. */
export function capOutput<T>(value: T): T {
  const walk = (item: unknown, depth: number): unknown => {
    if (typeof item === "string")
      return item.length > OUTPUT_STRING_MAX
        ? `${item.slice(0, OUTPUT_STRING_MAX)}\n[truncated, ${item.length - OUTPUT_STRING_MAX} more characters]`
        : item;
    if (item === null || typeof item !== "object" || depth > 24) return item;
    if (Array.isArray(item)) return item.map((entry) => walk(entry, depth + 1));
    return Object.fromEntries(
      Object.entries(item).map(([key, entry]) => [key, walk(entry, depth + 1)]),
    );
  };
  return walk(value, 0) as T;
}

const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

const clip = (text: string) =>
  text.length > ERROR_MAX ? `…${text.slice(-ERROR_MAX)}` : text;

function failed(error: unknown): ControlOutcome {
  return { status: "failed", text: "", error: message(error) };
}

/** "<workflow name> · 2026-10-22 14:05", within the folder name limit. */
export function runFolderName(workflowName: string, at: number): string {
  const date = new Date(at);
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const suffix = ` · ${stamp}`;
  const name = workflowName.replace(/\s+/g, " ").trim() || "Workflow";
  return `${name.slice(0, 100 - suffix.length)}${suffix}`;
}

function sleep(ms: number) {
  return new Promise<void>((resolve) => setTimeout(resolve, ms));
}

function withTimeout<T>(promise: Promise<T>, ms: number, what: string) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(what)), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

/**
 * Drives one run to its end: executes what the engine asks for, feeds the results back and
 * writes the run after every change. Resolves with the run as it ended. Never throws.
 */
export async function executeRun(
  getHost: () => MonoWorkflowHost,
  monoId: string,
  monoSessionId: string,
  initial: WorkflowRun,
): Promise<WorkflowRun> {
  let run = initial;
  const workflowId = run.workflowId;
  const active = new Set<Promise<void>>();
  let stopping = false;
  let nudged = false;
  let wake: (() => void) | undefined;
  const nudge = () => {
    nudged = true;
    wake?.();
  };
  const live = () => run.status === "running";

  // --- persistence ----------------------------------------------------------------------------
  // Every write fires a files-changed event, so writes go through one queue that always saves
  // the newest state; changes that arrive during a write are folded into the next one.
  let dirty = false;
  let writing: Promise<void> | undefined;
  const writeOnce = async () => {
    const latest = run;
    try {
      await updateRuns(monoId, workflowId, (runs) => ({
        runs: runs.some((entry) => entry.id === latest.id)
          ? runs.map((entry) => (entry.id === latest.id ? latest : entry))
          : [latest, ...runs],
        result: undefined,
      }));
    } catch (error) {
      console.error(`Could not save workflow run ${latest.id}`, error);
      // The run goes on in memory; the record says its history is incomplete.
      if (!run.error?.startsWith("Could not save"))
        run = {
          ...run,
          error: [
            `Could not save the run history: ${message(error)}`,
            run.error,
          ]
            .filter(Boolean)
            .join(" "),
        };
    }
  };
  const flush = async () => {
    try {
      while (dirty) {
        dirty = false;
        await writeOnce();
      }
    } finally {
      writing = undefined;
    }
  };
  const save = (): Promise<void> => {
    dirty = true;
    writing ??= flush();
    return writing;
  };

  const apply = (event: EngineEvent) => {
    run = applyEvent(run, event);
    void save();
  };
  const patchNode = (
    nodeId: NodeId,
    branch: BranchKey,
    patch: Partial<WorkflowRun["nodes"][number]>,
  ) => {
    run = {
      ...run,
      nodes: run.nodes.map((n) =>
        n.nodeId === nodeId && n.branch === branch ? { ...n, ...patch } : n,
      ),
    };
    void save();
  };
  const fail = (nodeId: NodeId, branch: BranchKey, error: string) =>
    apply({
      kind: "nodeFailed",
      nodeId,
      branch,
      error: clip(error),
      at: Date.now(),
    });

  // --- helpers --------------------------------------------------------------------------------
  const nodeOf = (nodeId: NodeId) =>
    run.definition.nodes.find((n) => n.id === nodeId);
  /** The session a node ran in this branch, or at the top level. */
  const sessionOf = (nodeId: NodeId, branch: BranchKey) =>
    (
      run.nodes.find(
        (n) => n.nodeId === nodeId && n.branch === branch && n.sessionId,
      ) ??
      run.nodes.find(
        (n) => n.nodeId === nodeId && n.branch === "" && n.sessionId,
      )
    )?.sessionId;
  const projectFor = (
    node: { project: string },
    nodeId: NodeId,
    branch: BranchKey,
  ): string | undefined =>
    getHost().projectPath(
      monoId,
      renderTemplate(node.project, runScope(run, branch, nodeId)),
    );

  /** The run's folder in each project it has started a session in. */
  const folderIds = new Map<string, string>();
  // Sessions are started one at a time so the first one makes the run's folder and the rest reuse it.
  let startChain: Promise<unknown> = Promise.resolve();
  const serial = <T>(task: () => Promise<T>): Promise<T> => {
    const next = startChain.then(task, task);
    startChain = next.catch(() => undefined);
    return next;
  };

  /**
   * Waits for a turn, relaying any approval it asks for to the Mono's chat. Only time spent
   * working counts toward the timeout; time waiting on the user has its own, longer limit.
   */
  const watchTurn = async (
    nodeId: NodeId,
    branch: BranchKey,
    sessionId: string,
    turn: Promise<ControlOutcome>,
    timeoutMs: number,
  ): Promise<ControlOutcome> => {
    let settled: ControlOutcome | undefined;
    const watched = turn.then(
      (outcome) => (settled = outcome),
      (error: unknown) => (settled = failed(error)),
    );
    let working = 0;
    let waiting = 0;
    let asking = false;
    try {
      while (!settled) {
        await Promise.race([watched, sleep(POLL_MS)]);
        if (settled) break;
        if (!live())
          return { status: "cancelled", text: "", error: STOPPED_BY_USER };
        const host = getHost();
        const pending =
          host
            .sessions()
            .find((entry) => entry.id === sessionId)
            ?.blocks.filter(
              (block) => block.approval && !block.approval.decided,
            ) ?? [];
        for (const block of pending)
          if (!isRelayed(sessionId, block.approval!.requestId))
            host.askApproval(
              monoSessionId,
              run.definition,
              run.id,
              sessionId,
              block,
            );
        if (pending.length > 0 !== asking) {
          asking = pending.length > 0;
          apply({ kind: "waitingApproval", nodeId, branch, waiting: asking });
        }
        if (pending.length) waiting += POLL_MS;
        else working += POLL_MS;
        if (working >= timeoutMs || waiting >= HABIT_APPROVAL_WAIT_MS) {
          const timedOut: ControlOutcome = {
            status: "failed",
            text: "",
            error:
              waiting >= HABIT_APPROVAL_WAIT_MS
                ? "Nobody answered its approval request."
                : `It took longer than ${Math.round(timeoutMs / 60_000)} minutes and was stopped.`,
          };
          await host.stop(sessionId).catch(() => undefined);
          return timedOut;
        }
      }
      return settled;
    } finally {
      if (asking && live())
        apply({ kind: "waitingApproval", nodeId, branch, waiting: false });
      getHost().endApprovals(sessionId);
    }
  };

  // --- node execution -------------------------------------------------------------------------
  const runSession = async (
    action: Extract<EngineAction, { kind: "startSession" }>,
  ) => {
    const { nodeId, branch } = action;
    const node = nodeOf(nodeId) as SessionNode;
    const host = getHost();
    const project = projectFor(node, nodeId, branch);
    if (!project)
      return fail(
        nodeId,
        branch,
        `"${node.project}" is not one of the Mono's projects`,
      );
    const timeoutMs = (node.timeoutMinutes ?? SESSION_TIMEOUT_MINUTES) * 60_000;

    let sessionId: string;
    let turn: Promise<ControlOutcome>;
    if (action.reuseSessionId) {
      sessionId = action.reuseSessionId;
      turn = host.send(sessionId, action.prompt);
    } else {
      let worktreeCwd: string | undefined;
      if (typeof node.checkout === "object") {
        const source = sessionOf(node.checkout.worktreeOf, branch);
        worktreeCwd = source ? host.worktreeOf(source) : undefined;
        if (!worktreeCwd)
          return fail(
            nodeId,
            branch,
            `No worktree is available from "${node.checkout.worktreeOf}"`,
          );
      }
      const parent = node.nestUnder
        ? sessionOf(node.nestUnder, branch)
        : undefined;
      const started = await serial(async () => {
        // Starts queued behind another one wait here; a stopped run must not launch them.
        if (!live()) return undefined;
        return host.startSession(monoId, {
          project,
          prompt: action.prompt,
          ...(node.harness ? { harness: node.harness } : {}),
          ...(node.model ? { model: node.model } : {}),
          ...(node.effort ? { effort: node.effort } : {}),
          workspaceMode:
            node.checkout === "newWorktree" ? "worktree" : "current",
          ...(worktreeCwd ? { worktreeCwd } : {}),
          ...(parent ? { parentSessionId: parent } : {}),
          // A folder belongs to one project: a project the run has not filed in yet gets the
          // first one's id with the name, which makes a folder there (and is remembered below).
          ...((folderIds.get(project) ?? run.folderId)
            ? { folderId: folderIds.get(project) ?? run.folderId }
            : {}),
          folderName: runFolderName(run.definition.name, run.startedAt),
        });
      });
      if (!started) return;
      sessionId = started.sessionId;
      if (started.folderId) {
        folderIds.set(project, started.folderId);
        if (!run.folderId) {
          run = { ...run, folderId: started.folderId };
          void save();
        }
      }
      turn = started.settled;
      if (!live()) {
        // Stopped while it was starting: nothing else knows this session yet.
        await host.stop(sessionId).catch(() => undefined);
        return;
      }
    }
    patchNode(nodeId, branch, { sessionId });

    let outcome = await watchTurn(nodeId, branch, sessionId, turn, timeoutMs);
    if (!live()) return;
    if (outcome.status !== "completed")
      return fail(
        nodeId,
        branch,
        outcome.error ??
          (outcome.status === "cancelled"
            ? "The session was stopped"
            : "The session did not finish"),
      );
    const replyOf = (done: ControlOutcome) =>
      finalReply(host.sessions().find((entry) => entry.id === sessionId)) ||
      done.text;

    const fields = node.output;
    // A node with nothing declared has no contract to enforce, whatever its reply ends with.
    if (Object.keys(fields).length === 0)
      return apply({
        kind: "nodeFinished",
        nodeId,
        branch,
        output: {},
        at: Date.now(),
      });

    let parsed = parseWorkflowOutput(replyOf(outcome), fields);
    if (!parsed.ok) {
      patchNode(nodeId, branch, { reasked: true });
      outcome = await watchTurn(
        nodeId,
        branch,
        sessionId,
        host.send(sessionId, outputReask(fields, parsed.error)),
        timeoutMs,
      );
      if (!live()) return;
      if (outcome.status !== "completed")
        return fail(nodeId, branch, outcome.error ?? parsed.error);
      parsed = parseWorkflowOutput(replyOf(outcome), fields);
    }
    if (!parsed.ok) return fail(nodeId, branch, parsed.error);
    apply({
      kind: "nodeFinished",
      nodeId,
      branch,
      output: capOutput(parsed.value),
      at: Date.now(),
    });
  };

  const runCommand = async (
    action: Extract<EngineAction, { kind: "runCommand" }>,
  ) => {
    const { nodeId, branch } = action;
    const node = nodeOf(nodeId);
    if (node?.type !== "command") return;
    const host = getHost();
    let cwd = projectFor(node, nodeId, branch);
    if (!cwd)
      return fail(
        nodeId,
        branch,
        `"${node.project}" is not one of the Mono's projects`,
      );
    if (typeof node.cwd === "object") {
      const source = sessionOf(node.cwd.worktreeOf, branch);
      cwd = source ? host.worktreeOf(source) : undefined;
      if (!cwd)
        return fail(
          nodeId,
          branch,
          `No worktree is available from "${node.cwd.worktreeOf}"`,
        );
    }
    const minutes = node.timeoutMinutes ?? COMMAND_TIMEOUT_MINUTES;
    const result = await host.runCommand(cwd, action.command, minutes * 60_000);
    if (!live()) return;
    if (result.timedOut)
      return fail(
        nodeId,
        branch,
        `The command timed out after ${minutes} minutes`,
      );
    if (result.exitCode !== 0) {
      const detail = result.stderr.trim() || result.stdout.trim();
      return fail(
        nodeId,
        branch,
        `The command exited with code ${result.exitCode ?? "unknown"}${detail ? `: ${detail.slice(-500)}` : ""}`,
      );
    }
    apply({
      kind: "nodeFinished",
      nodeId,
      branch,
      output: capOutput({
        exitCode: result.exitCode,
        stdout: result.stdout,
        stderr: result.stderr,
      }),
      at: Date.now(),
    });
  };

  const runAsk = async (action: Extract<EngineAction, { kind: "askMono" }>) => {
    const { nodeId, branch } = action;
    const reply = await withTimeout(
      getHost().askMono(monoId, askMonoPrompt(action, run.definition.name)),
      ASK_TIMEOUT_MS,
      "The Mono did not answer in time",
    );
    if (!live()) return;
    const decision = parseMonoDecision(reply, action.answers);
    if (!decision.ok) return fail(nodeId, branch, decision.error);
    apply({
      kind: "decided",
      nodeId,
      branch,
      value: decision.value,
      ...(decision.reason !== undefined
        ? { reason: capOutput(decision.reason) }
        : {}),
      at: Date.now(),
    });
  };

  const execute = async (action: EngineAction) => {
    if (action.kind === "startSession") return runSession(action);
    if (action.kind === "runCommand") return runCommand(action);
    if (action.kind === "askMono") return runAsk(action);
  };

  const track = (action: Exclude<EngineAction, { kind: "postReport" }>) => {
    const task: Promise<void> = execute(action)
      .catch((error: unknown) => {
        console.warn(`Workflow node "${action.nodeId}" failed`, error);
        // A loop question has no running work to fail; the engine fails the node all the same.
        if (live()) fail(action.nodeId, action.branch, message(error));
      })
      .finally(() => {
        active.delete(task);
        nudge();
      });
    active.add(task);
  };

  const launchesOf = (finished: WorkflowRun): MonoSpawnedSession[] => {
    const seen = new Set<string>();
    const launches: MonoSpawnedSession[] = [];
    const open = getHost().sessions();
    for (const n of finished.nodes) {
      if (!n.sessionId || seen.has(n.sessionId)) continue;
      seen.add(n.sessionId);
      const found = open.find((entry) => entry.id === n.sessionId);
      if (found)
        launches.push({
          sessionId: found.id,
          cwd: found.cwd,
          title: found.title,
          harness: found.harness,
          model: found.model,
        });
    }
    return launches;
  };

  const postText = (finished: WorkflowRun) => {
    const { title, text } = composeReport(finished);
    try {
      getHost().post(
        monoSessionId,
        finished.definition,
        finished.id,
        title,
        text,
        launchesOf(finished),
      );
    } catch (error) {
      console.error("Could not post the workflow report", error);
    }
  };

  /** Posts the report. The run's final status is known once the report counts as posted. */
  const postReport = (): boolean => {
    const posted = applyEvent(run, { kind: "reportPosted", at: Date.now() });
    if (posted === run) return false;
    run = posted;
    void save();
    postText(posted);
    return true;
  };

  /** Starts everything the engine has ready. A question or session is marked started first. */
  const pump = () => {
    for (let i = 0; i < 1_000 && live(); i++) {
      const action = nextActions(run)[0];
      if (!action) return;
      if (action.kind === "postReport") {
        if (!postReport()) return;
        continue;
      }
      const before = run;
      apply({
        kind: "nodeStarted",
        nodeId: action.nodeId,
        branch: action.branch,
        ...(action.kind === "startSession" && action.reuseSessionId
          ? { sessionId: action.reuseSessionId }
          : {}),
        at: Date.now(),
      });
      // The engine refused: do not loop on an action it will keep offering.
      if (run === before) return;
      track(action);
    }
  };

  const stop = async () => {
    if (stopping || !live()) return;
    stopping = true;
    const sessions = run.nodes
      .filter(
        (n) =>
          (n.status === "running" || n.status === "waitingApproval") &&
          !isAsking(n) &&
          n.sessionId,
      )
      .map((n) => n.sessionId as string);
    // The run is stopped first, so whatever the sessions report as they stop changes nothing.
    apply({ kind: "stopped", at: Date.now() });
    nudge();
    const host = getHost();
    await Promise.all(
      [...new Set(sessions)].map(async (id) => {
        try {
          await host.stop(id);
        } catch (error) {
          console.warn(`Could not stop session ${id}`, error);
        }
        host.endApprovals(id);
      }),
    );
    // A stopped run reports once, with what had finished by then.
    if (run.reportPostedAt === undefined) {
      run = { ...run, reportPostedAt: Date.now() };
      void save();
      postText(run);
    }
  };

  let stopTask: Promise<void> | undefined;
  const listen = () => {
    if (workflowStopRequested(workflowId)) stopTask ??= stop();
  };
  const unsubscribe = subscribeWorkflowsRunning(listen);

  // A run is its window's to look after for as long as it lives, whatever that window's place
  // in the election: it tells the others it is going, and applies a stop written to the file.
  const registry = liveRunRegistry();
  const beat = () => {
    if (live()) registry?.touch(run.id, workflowId);
  };
  let looking = false;
  const lookForStop = async () => {
    if (!live() || looking) return;
    looking = true;
    try {
      const asked = (await loadWorkflows(monoId)).find(
        (w) => w.id === workflowId,
      )?.stopRequested;
      if (!asked || !live()) return;
      requestWorkflowStop(workflowId);
      await updateWorkflows(monoId, (list) => ({
        workflows: list.map((w) => {
          const { stopRequested: _stop, ...rest } = w;
          return w.id === workflowId ? rest : w;
        }),
        result: undefined,
      }));
    } catch (error) {
      console.warn("Could not check for a workflow stop request", error);
    } finally {
      looking = false;
    }
  };
  const timers = [
    setInterval(beat, LIVE_RUN_REFRESH_MS),
    setInterval(() => void lookForStop(), STOP_CHECK_MS),
  ];
  beat();

  try {
    // The record exists before anything runs, so a failure to save is known up front.
    await save();
    listen();
    while (live()) {
      pump();
      if (!live()) break;
      if (active.size === 0) {
        run = {
          ...run,
          status: "failed",
          endedAt: Date.now(),
          error: [run.error, "The run could not make progress."]
            .filter(Boolean)
            .join(" "),
        };
        void save();
        break;
      }
      if (!nudged) await new Promise<void>((resolve) => (wake = resolve));
      wake = undefined;
      nudged = false;
      listen();
    }
  } catch (error) {
    console.error("A workflow run stopped unexpectedly", error);
    if (live()) {
      run = {
        ...run,
        status: "failed",
        endedAt: Date.now(),
        error: message(error),
      };
      void save();
    }
  } finally {
    unsubscribe();
    for (const timer of timers) clearInterval(timer);
    try {
      await stopTask;
      await writing;
    } catch (error) {
      console.warn("A workflow run did not finish cleanly", error);
    }
    // These must happen whatever went wrong above or in the approvals clean-up below.
    registry?.remove(run.id);
    try {
      for (const n of run.nodes)
        if (n.sessionId) getHost().endApprovals(n.sessionId);
    } catch (error) {
      console.warn("Could not withdraw a workflow run's approvals", error);
    } finally {
      setWorkflowRunning(workflowId, null);
      clearWorkflowStop(workflowId);
    }
  }
  return run;
}
