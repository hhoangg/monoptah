import { useState, type CSSProperties } from "react";
import { resumeWorkflowRun } from "../../../app/hooks/useMonoWorkflows";
import { IconButton } from "../../../app/shell/TitleBar";
import { Play, Square } from "../../../shared/ui/icons";
import { duration, RunningFor, when } from "../../monos/ui/HabitPage";
import { PageHeader, Property, Section } from "../../monos/ui/monoPanelParts";
import {
  archivedItemsKey,
  isTrigger,
  parseBranch,
  type NodeRun,
  type NodeStatus,
  type RunStatus,
  type Workflow,
  type WorkflowNode,
  type WorkflowRun,
} from "../model/workflow";
import { isAsking } from "../model/workflowEngine";
import { stopWorkflowRun } from "../model/workflowStore";
import { liveRunOf, useWorkflowRuns, WorkflowNotice } from "./MonoWorkflows";

/** Dot colours reuse the habit page's: the Mono's colour, red for trouble, grey for rest. */
export const RUN_STATUS: Record<RunStatus, { label: string; dot: string }> = {
  running: {
    label: "Running",
    dot: "animate-pulse bg-[var(--mono-color)]",
  },
  done: { label: "Finished", dot: "bg-emerald-400/80" },
  doneWithFailures: {
    label: "Finished with failures",
    dot: "bg-amber-400/80",
  },
  failed: { label: "Failed", dot: "bg-red-500/70" },
  stopped: { label: "Stopped", dot: "bg-content/40" },
  interrupted: { label: "Interrupted", dot: "bg-amber-400/80" },
};

const NODE_STATUS: Record<NodeStatus, { label: string; dot: string }> = {
  pending: { label: "Waiting", dot: "bg-content/25" },
  running: {
    label: "Running",
    dot: "animate-pulse bg-[var(--mono-color)]",
  },
  waitingApproval: {
    label: "Waiting for approval",
    dot: "bg-amber-400/80",
  },
  done: { label: "Done", dot: "bg-emerald-400/80" },
  failed: { label: "Failed", dot: "bg-red-500/70" },
  skipped: { label: "Skipped", dot: "bg-content/25" },
  stopped: { label: "Stopped", dot: "bg-content/40" },
};

/** The parts of a definition that change what a run does; board positions do not. */
function definitionKey(workflow: Workflow): string {
  return JSON.stringify(
    {
      inputs: workflow.inputs,
      nodes: workflow.nodes.map(({ ui: _ui, ...node }) => node),
      edges: workflow.edges,
    },
    // Sorted keys, so two definitions that read the same compare the same.
    (_key, value: unknown) =>
      value && typeof value === "object" && !Array.isArray(value)
        ? Object.fromEntries(
            Object.entries(value).sort(([a], [b]) => (a < b ? -1 : 1)),
          )
        : value,
  );
}

/** Whether the workflow was edited after the run took its copy. */
export function definitionChanged(run: WorkflowRun, current: Workflow): boolean {
  return definitionKey(run.definition) !== definitionKey(current);
}

type Lane = { branch: string; title: string; nodes: NodeRun[] };

/** A branch key as the lane list needs it: `round` is set on a branch archived by a loop round. */
function splitBranch(branch: string): {
  groupId: string;
  index: number;
  round?: number;
} {
  return parseBranch(branch) ?? { groupId: branch, index: 0 };
}

/** An item's name for its lane: its title or name when it has one, else its place. */
function itemTitle(run: WorkflowRun, branch: string): string {
  const { groupId, index, round } = splitBranch(branch);
  const items = run.items[round === undefined ? groupId : archivedItemsKey(groupId, round)];
  const item = items?.[index];
  const suffix = round === undefined ? "" : ` · round ${round}`;
  if (item && typeof item === "object") {
    const fields = item as Record<string, unknown>;
    for (const key of ["title", "name"]) {
      const value = fields[key];
      if (typeof value === "string" && value.trim()) return `${value.trim()}${suffix}`;
    }
  }
  return `#${index + 1}${suffix}`;
}

/** One lane per branch: the top level first, then each For each item in order. */
export function lanesOf(run: WorkflowRun): Lane[] {
  const order = new Map(run.definition.nodes.map((n, i) => [n.id, i]));
  const byId = new Map<string, WorkflowNode>(
    run.definition.nodes.map((n) => [n.id, n]),
  );
  const shown = run.nodes
    .filter((n) => {
      const node = byId.get(n.nodeId);
      return !node || !isTrigger(node);
    })
    .sort((a, b) => (order.get(a.nodeId) ?? 0) - (order.get(b.nodeId) ?? 0));
  const branches = [...new Set(shown.map((n) => n.branch))].sort((a, b) => {
    if (a === "" || b === "") return a === "" ? -1 : 1;
    const left = splitBranch(a);
    const right = splitBranch(b);
    // Earlier loop rounds of an item come right before its live lane.
    return (
      (order.get(left.groupId) ?? 0) - (order.get(right.groupId) ?? 0) ||
      left.index - right.index ||
      (left.round ?? Infinity) - (right.round ?? Infinity)
    );
  });
  return branches.map((branch) => ({
    branch,
    title: branch === "" ? "Workflow" : itemTitle(run, branch),
    nodes: shown.filter((n) => n.branch === branch),
  }));
}

function NodeChip({
  run,
  node,
  onOpenSession,
}: {
  run: WorkflowRun;
  node: NodeRun;
  onOpenSession: (sessionId: string) => void | Promise<void>;
}) {
  const def = run.definition.nodes.find((n) => n.id === node.nodeId);
  const status = NODE_STATUS[node.status];
  const detail = [
    isAsking(node) ? "Deciding what's next" : status.label,
    node.status === "failed" ? node.error : undefined,
    node.decision ? (node.decision.reason ?? node.decision.value) : undefined,
  ].filter(Boolean);
  const body = (
    <>
      <span
        aria-hidden
        className={`size-2 shrink-0 rounded-full ${status.dot}`}
        data-node-status={node.status}
      />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate text-[12px] leading-5 text-content/85">
          {def?.title ?? node.nodeId}
          {node.round > 1 ? ` · round ${node.round}` : ""}
        </span>
        <span className="truncate text-[11px] leading-4 text-content/40">
          {detail.join(" · ")}
        </span>
      </span>
    </>
  );
  const chip =
    "flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left";
  return (
    <li>
      {node.sessionId ? (
        <button
          type="button"
          data-node={node.nodeId}
          className={`${chip} hover:bg-content/5`}
          onClick={() => onOpenSession(node.sessionId as string)}
        >
          {body}
        </button>
      ) : (
        <div data-node={node.nodeId} className={chip}>
          {body}
        </div>
      )}
    </li>
  );
}

/** One run up close: a lane for each branch, with the nodes it passed and their sessions. */
export function WorkflowRunPage({
  monoId,
  workflowId,
  runId,
  current,
  color,
  onBack,
  onOpenSession,
}: {
  monoId: string;
  workflowId: string;
  runId: string;
  /** The workflow as it is now, to tell whether it changed since the run began. */
  current?: Workflow;
  color: string;
  onBack: () => void;
  onOpenSession: (sessionId: string) => void | Promise<void>;
}) {
  const runs = useWorkflowRuns(monoId, workflowId);
  const [notice, setNotice] = useState<string>();
  const run = runs?.find((entry) => entry.id === runId);
  const status = run ? RUN_STATUS[run.status] : undefined;
  const openSession = (sessionId: string) => {
    setNotice(undefined);
    const failed = () => setNotice("Could not open this session.");
    try {
      void Promise.resolve(onOpenSession(sessionId)).catch(failed);
    } catch {
      failed();
    }
  };
  const resume = () => {
    // Two runs of one workflow would race over its files; this one waits for the other.
    if (liveRunOf(workflowId))
      return setNotice(
        "Another run of this workflow is going. Resume this one when it ends.",
      );
    setNotice(undefined);
    void resumeWorkflowRun(monoId, workflowId, runId).catch((error) =>
      console.warn("Could not resume the run", error),
    );
  };
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      style={{ "--mono-color": color } as CSSProperties}
      data-workflow-run-page={runId}
    >
      <PageHeader title={run?.definition.name ?? "Run"} onBack={onBack}>
        {run?.status === "running" ? (
          <IconButton
            label="Stop run"
            onClick={() =>
              void stopWorkflowRun(monoId, workflowId).catch((error) =>
                console.warn("Could not stop the run", error),
              )
            }
          >
            <Square className="size-3.5" strokeWidth={1.75} />
          </IconButton>
        ) : null}
        {run?.status === "interrupted" ? (
          <IconButton
            label="Resume"
            onClick={resume}
          >
            <Play className="size-3.5" strokeWidth={1.75} />
          </IconButton>
        ) : null}
      </PageHeader>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
        <WorkflowNotice>{notice}</WorkflowNotice>
        {run && status ? (
          <>
            <dl className="flex flex-col gap-0.5 px-4 py-3">
              <Property label="Status">
                <span
                  className={`mr-2 size-2 shrink-0 rounded-full ${status.dot}`}
                />
                {status.label}
              </Property>
              <Property label="Started">{when(run.startedAt)}</Property>
              <Property label="Took">
                {run.endedAt ? (
                  duration(run.endedAt - run.startedAt)
                ) : run.status === "running" ? (
                  <RunningFor since={run.startedAt} />
                ) : (
                  "-"
                )}
              </Property>
            </dl>
            {current && definitionChanged(run, current) ? (
              <p
                data-run-changed
                className="px-4 pb-3 text-[11px] leading-4 text-content/45"
              >
                The workflow changed since this run started.
              </p>
            ) : null}
            {run.error ? (
              <p className="whitespace-pre-wrap px-4 pb-3 text-[12px] leading-5 text-red-500/80">
                {run.error}
              </p>
            ) : null}
            {lanesOf(run).map((lane) => (
              <div key={lane.branch} data-lane={lane.branch}>
                <Section title={lane.title}>
                  <ul className="flex flex-col gap-px">
                    {lane.nodes.map((node) => (
                      <NodeChip
                        key={node.nodeId}
                        run={run}
                        node={node}
                        onOpenSession={openSession}
                      />
                    ))}
                  </ul>
                </Section>
              </div>
            ))}
          </>
        ) : runs ? (
          <p className="px-4 py-6 text-center text-[12px] text-content/45">
            This run is no longer kept.
          </p>
        ) : null}
      </div>
    </div>
  );
}
