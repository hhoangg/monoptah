import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Pause,
  Play,
  Square,
  Trash2,
  Workflow as WorkflowIcon,
  Zap,
} from "../../../shared/ui/icons";
import { liveRunRegistry } from "../../../app/hooks/workflowLiveRuns";
import type { MonoState } from "../../monos/model/mono";
import { subscribeMonoFiles } from "../../monos/model/monoFiles";
import {
  habitScheduleLabel,
  nextHabitRunAt,
} from "../../monos/model/monoHabits";
import { RunningFor } from "../../monos/ui/HabitPage";
import { HabitButton } from "../../monos/ui/MonoHabits";
import { Hint, PageHeader, Empty } from "../../monos/ui/monoPanelParts";
import {
  triggerOf,
  type Workflow,
  type WorkflowId,
  type WorkflowRun,
} from "../model/workflow";
import { validateWorkflow, type WorkflowIssue } from "../model/workflowParse";
import {
  checkWorkflowsNow,
  loadRuns,
  loadWorkflows,
  stopWorkflowRun,
  subscribeWorkflowsRunning,
  updateWorkflows,
  workflowRunningId,
} from "../model/workflowStore";

/** How often a run in progress is looked at again. */
const RUN_POLL_MS = 2_000;

/** Each Mono's workflows as last loaded, so switching back shows them at once. */
const loadedWorkflows = new Map<string, Workflow[]>();

/** The Mono's workflows, reloaded whenever its folder changes or a run starts or ends. */
export function useWorkflows(
  monoId: string,
  status: MonoState["status"],
): Workflow[] | undefined {
  const [workflows, setWorkflows] = useState<Workflow[] | undefined>(() =>
    loadedWorkflows.get(monoId),
  );
  const working = status === "working";
  useEffect(() => {
    let live = true;
    const refresh = () => {
      void loadWorkflows(monoId)
        .then((next) => {
          loadedWorkflows.set(monoId, next);
          if (live) setWorkflows(next);
        })
        .catch(() => {});
    };
    refresh();
    const stop = subscribeMonoFiles(refresh);
    const stopRuns = subscribeWorkflowsRunning(refresh);
    window.addEventListener("focus", refresh);
    // Runs move nextRunAt along without a panel event; check now and then.
    const timer = window.setInterval(refresh, 30_000);
    return () => {
      live = false;
      stop();
      stopRuns();
      window.removeEventListener("focus", refresh);
      window.clearInterval(timer);
    };
  }, [monoId, working]);
  return workflows;
}

/** The run of this workflow going in some window, if any. */
export function liveRunOf(workflowId: WorkflowId): string | undefined {
  return (
    workflowRunningId(workflowId) ?? liveRunRegistry()?.freshRunOf(workflowId)
  );
}

/** The id of the workflow's running run, kept current as runs start and end. */
export function useLiveRunId(workflowId: WorkflowId): string | undefined {
  const [runId, setRunId] = useState(() => liveRunOf(workflowId));
  useEffect(() => {
    const sync = () => setRunId(liveRunOf(workflowId));
    sync();
    const stop = subscribeWorkflowsRunning(sync);
    // A run another window drives is only seen through the registry.
    const timer = window.setInterval(sync, RUN_POLL_MS);
    return () => {
      stop();
      window.clearInterval(timer);
    };
  }, [workflowId]);
  return runId;
}

/** Each workflow's runs as last loaded. */
const loadedRuns = new Map<string, WorkflowRun[]>();

/**
 * A workflow's runs, newest first. Reloaded when the Mono's files change (the app saves every
 * step of a run there) and every 2 s while a run is going. Undefined until loaded, and always
 * for a caller that has `enabled` off.
 */
export function useWorkflowRuns(
  monoId: string,
  workflowId: WorkflowId,
  enabled = true,
): WorkflowRun[] | undefined {
  const key = `${monoId}/${workflowId}`;
  const [runs, setRuns] = useState<WorkflowRun[] | undefined>(() =>
    enabled ? loadedRuns.get(key) : undefined,
  );
  const going = useRef(false);
  going.current = runs?.some((run) => run.status === "running") ?? false;
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    const refresh = () => {
      void loadRuns(monoId, workflowId)
        .then((next) => {
          loadedRuns.set(key, next);
          if (live) setRuns(next);
        })
        .catch(() => {});
    };
    refresh();
    const stop = subscribeMonoFiles(refresh);
    const stopRuns = subscribeWorkflowsRunning(refresh);
    window.addEventListener("focus", refresh);
    const timer = window.setInterval(() => {
      if (going.current || liveRunOf(workflowId)) refresh();
    }, RUN_POLL_MS);
    return () => {
      live = false;
      stop();
      stopRuns();
      window.removeEventListener("focus", refresh);
      window.clearInterval(timer);
    };
  }, [monoId, workflowId, key, enabled]);
  return enabled ? runs : undefined;
}

/** The run that is going, among a workflow's runs. */
export function runningRunOf(
  runs: readonly WorkflowRun[] | undefined,
  liveRunId: string | undefined,
): WorkflowRun | undefined {
  return (
    runs?.find((run) => run.id === liveRunId) ??
    runs?.find((run) => run.status === "running")
  );
}

/** Branches with a node at work: the group node itself is bookkeeping, not a branch. */
export function activeBranches(run: WorkflowRun): number {
  const groups = new Set(
    run.definition.nodes.filter((n) => n.type === "forEach").map((n) => n.id),
  );
  const branches = new Set(
    run.nodes
      .filter(
        (n) =>
          (n.status === "running" || n.status === "waitingApproval") &&
          !groups.has(n.nodeId),
      )
      .map((n) => n.branch),
  );
  return Math.max(1, branches.size);
}

/** "Weekdays 09:00" for a schedule trigger, "Manual" otherwise. */
export function workflowTriggerLabel(workflow: Workflow): string {
  const trigger = triggerOf(workflow);
  return trigger?.type === "schedule"
    ? habitScheduleLabel(trigger.schedule)
    : "Manual";
}

/** Why a workflow cannot be run or resumed yet, in the words shown under the list. */
function refusal(verb: string, issues: WorkflowIssue[]): string {
  const [first, ...rest] = issues;
  const reason = first.message.replace(/[.\s]+$/, "");
  return `Can't ${verb} yet: ${reason}${rest.length ? ` (and ${rest.length} more)` : ""}.`;
}

export type WorkflowActions = {
  /** Resolves to why the workflow cannot run yet, if it cannot. */
  runNow(id: string): Promise<string | undefined>;
  stop(id: string): Promise<void>;
  /** Resolves to why a paused workflow cannot be resumed yet, if it cannot. */
  toggle(id: string): Promise<string | undefined>;
  remove(id: string): Promise<void>;
};

/** Edits to the Mono's workflows from the panel, saved over what was read. */
export function workflowActions(monoId: string): WorkflowActions {
  const change = async (
    id: string,
    edit: (workflow: Workflow) => { next: Workflow | null; notice?: string },
  ): Promise<string | undefined> => {
    try {
      return await updateWorkflows(monoId, (list) => {
        let notice: string | undefined;
        const workflows = list.flatMap((workflow) => {
          if (workflow.id !== id) return [workflow];
          const result = edit(workflow);
          notice = result.notice;
          return result.next ? [result.next] : [];
        });
        return { workflows, result: notice };
      });
    } catch (error) {
      console.warn("Could not change the workflow", error);
      return "Could not change the workflow.";
    }
  };
  return {
    runNow: async (id) => {
      const notice = await change(id, (workflow) => {
        const issues = validateWorkflow(workflow);
        return issues.length
          ? { next: workflow, notice: refusal("run", issues) }
          : { next: { ...workflow, runRequested: true } };
      });
      if (!notice) checkWorkflowsNow();
      return notice;
    },
    stop: async (id) => {
      await stopWorkflowRun(monoId, id).catch((error) =>
        console.warn("Could not stop the workflow", error),
      );
    },
    toggle: (id) =>
      change(id, (workflow) => {
        if (workflow.enabled)
          return { next: { ...workflow, enabled: false } };
        const issues = validateWorkflow(workflow);
        if (issues.length)
          return { next: workflow, notice: refusal("resume", issues) };
        const trigger = triggerOf(workflow);
        return {
          next: {
            ...workflow,
            enabled: true,
            ...(trigger?.type === "schedule"
              ? { nextRunAt: nextHabitRunAt(trigger.schedule, Date.now()) }
              : {}),
          },
        };
      }),
    remove: async (id) => {
      await change(id, () => ({ next: null }));
    },
  };
}

/** What a refused action says, under the list or a page's header. */
export function WorkflowNotice({ children }: { children?: ReactNode }) {
  if (!children) return null;
  return (
    <div data-workflow-notice role="status">
      <Hint>{children}</Hint>
    </div>
  );
}

export function WorkflowRow({
  monoId,
  workflow,
  actions,
  notify,
  onOpen,
}: {
  monoId: string;
  workflow: Workflow;
  actions: WorkflowActions;
  /** Told why an action was refused, or undefined when the next one went through. */
  notify: (message: string | undefined) => void;
  onOpen: () => void;
}) {
  const liveRunId = useLiveRunId(workflow.id);
  const running = liveRunId !== undefined;
  const runs = useWorkflowRuns(monoId, workflow.id, running);
  const current = running ? runningRunOf(runs, liveRunId) : undefined;
  const starting = !running && !!workflow.runRequested;
  const label = workflowTriggerLabel(workflow);
  return (
    <li
      data-workflow={workflow.id}
      data-workflow-running={running || undefined}
      className="group/workflow flex items-center rounded-md hover:bg-content/5"
    >
      <button
        type="button"
        onClick={onOpen}
        className="flex min-w-0 flex-1 items-center gap-2.5 rounded-md px-2 py-2 text-left"
      >
        <span
          aria-hidden
          className={`grid size-7 shrink-0 place-items-center rounded-md bg-content/6 ${
            workflow.enabled || running ? "text-content/60" : "text-content/30"
          }`}
        >
          {running ? (
            <span className="size-2 animate-pulse rounded-full bg-[var(--mono-color)]" />
          ) : (
            <WorkflowIcon className="size-3.5" strokeWidth={1.75} />
          )}
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span
            className={`truncate text-[12px] leading-5 ${
              workflow.enabled || running ? "text-content/85" : "text-content/45"
            }`}
          >
            {workflow.name}
          </span>
          <span className="truncate text-[11px] leading-4 text-content/40">
            {running ? (
              current ? (
                <>
                  Running · {activeBranches(current)}{" "}
                  {activeBranches(current) === 1 ? "branch" : "branches"} ·{" "}
                  <RunningFor since={current.startedAt} />
                </>
              ) : (
                "Running"
              )
            ) : starting ? (
              "Starting…"
            ) : workflow.enabled ? (
              label
            ) : (
              `Paused · ${label}`
            )}
          </span>
        </span>
      </button>
      <span className="flex shrink-0 items-center pr-2 opacity-0 transition-opacity group-hover/workflow:opacity-100 focus-within:opacity-100">
        <HabitButton
          label={
            running ? "Already running" : starting ? "Starting" : "Run now"
          }
          disabled={running || starting}
          onClick={() => void actions.runNow(workflow.id).then(notify)}
        >
          <Zap className="size-3.5" strokeWidth={1.75} />
        </HabitButton>
        {running ? (
          <HabitButton
            label="Stop"
            onClick={() => void actions.stop(workflow.id)}
          >
            <Square className="size-3.5" strokeWidth={1.75} />
          </HabitButton>
        ) : null}
        <HabitButton
          label={workflow.enabled ? "Pause" : "Resume"}
          onClick={() => void actions.toggle(workflow.id).then(notify)}
        >
          {workflow.enabled ? (
            <Pause className="size-3.5" strokeWidth={1.75} />
          ) : (
            <Play className="size-3.5" strokeWidth={1.75} />
          )}
        </HabitButton>
        <HabitButton
          label="Remove"
          onClick={() => void actions.remove(workflow.id)}
        >
          <Trash2 className="size-3.5" strokeWidth={1.75} />
        </HabitButton>
      </span>
    </li>
  );
}

export function WorkflowsList({
  monoId,
  workflows,
  actions,
  onOpen,
  empty = <Empty>No workflows yet</Empty>,
}: {
  monoId: string;
  workflows: Workflow[] | undefined;
  actions: WorkflowActions;
  onOpen: (id: string) => void;
  /** Shown once loaded with none. */
  empty?: ReactNode;
}) {
  const [notice, setNotice] = useState<string>();
  // Blank for the moment it loads, rather than a line the list replaces.
  if (!workflows) return null;
  if (workflows.length === 0) return <>{empty}</>;
  return (
    <div className="flex flex-col pt-1">
      <WorkflowNotice>{notice}</WorkflowNotice>
      <ul className="flex flex-col gap-px">
        {workflows.map((workflow) => (
          <WorkflowRow
            key={workflow.id}
            monoId={monoId}
            workflow={workflow}
            actions={actions}
            notify={setNotice}
            onOpen={() => onOpen(workflow.id)}
          />
        ))}
      </ul>
    </div>
  );
}

/**
 * A page whose editor has not arrived yet: the board, a new workflow and an import open
 * here until their own pages replace it.
 */
export function WorkflowPlaceholderPage({
  title,
  onBack,
}: {
  title: string;
  onBack: () => void;
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-workflow-placeholder>
      <PageHeader title={title} onBack={onBack} />
      <div className="min-h-0 flex-1 overflow-y-auto overscroll-none">
        <Empty>Not available yet</Empty>
      </div>
    </div>
  );
}
