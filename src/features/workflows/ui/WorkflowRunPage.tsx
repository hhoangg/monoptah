import { useState, type CSSProperties, type ReactNode } from "react";
import { resumeWorkflowRun } from "../../../app/hooks/useMonoWorkflows";
import { IconButton } from "../../../app/shell/TitleBar";
import { Columns3, Layers, Play, Square } from "../../../shared/ui/icons";
import { duration, RunningFor, when } from "../../monos/ui/HabitPage";
import { PageHeader } from "../../monos/ui/monoPanelParts";
import type { RunStatus, Workflow, WorkflowRun } from "../model/workflow";
import { stopWorkflowRun } from "../model/workflowStore";
import { liveRunOf, useWorkflowRuns, WorkflowNotice } from "./MonoWorkflows";
import { defaultSelection, hasForEach, RunGraph, useRunView } from "./run/RunGraph";
import { RunNodeDetails } from "./run/RunNodeDetails";
import { nodeRunsByKey } from "./run/runGraphModel";

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

function Meta({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex h-6 items-center gap-1.5">
      <dt className="text-content/45">{label}</dt>
      <dd className="flex items-center text-content/85">{children}</dd>
    </div>
  );
}

/** One run up close: the workflow as it ran, node by node, and what the picked node did. */
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
  const [picked, setPicked] = useState<string>();
  const run = runs?.find((entry) => entry.id === runId);
  const { view, toggle } = useRunView(workflowId, run);
  // A pick the run no longer has (a loop reset it, the run was replaced) falls back to the default.
  const selected = run
    ? picked && nodeRunsByKey(run).has(picked)
      ? picked
      : defaultSelection(run)
    : undefined;
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
    // The driver would pick it up from the file; the review comes first, in the panel.
    if (current?.needsReview)
      return setNotice("Review this workflow before resuming its run.");
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
        {run && hasForEach(run) ? (
          <IconButton
            label={view === "lanes" ? "Show one item at a time" : "Show items side by side"}
            onClick={toggle}
          >
            {view === "lanes" ? (
              <Layers className="size-3.5" strokeWidth={1.75} />
            ) : (
              <Columns3 className="size-3.5" strokeWidth={1.75} />
            )}
          </IconButton>
        ) : null}
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

      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-none">
        <WorkflowNotice>{notice}</WorkflowNotice>
        {run && status ? (
          <>
            <dl className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-0.5 px-4 py-2 text-[12px]">
              <Meta label="Status">
                <span
                  className={`mr-1.5 size-2 shrink-0 rounded-full ${status.dot}`}
                />
                {status.label}
              </Meta>
              <Meta label="Started">{when(run.startedAt)}</Meta>
              <Meta label="Took">
                {run.endedAt ? (
                  duration(run.endedAt - run.startedAt)
                ) : run.status === "running" ? (
                  <RunningFor since={run.startedAt} />
                ) : (
                  "-"
                )}
              </Meta>
            </dl>
            {current && definitionChanged(run, current) ? (
              <p
                data-run-changed
                className="shrink-0 px-4 pb-3 text-[11px] leading-4 text-content/45"
              >
                The workflow changed since this run started.
              </p>
            ) : null}
            {run.error ? (
              <p className="shrink-0 whitespace-pre-wrap px-4 pb-3 text-[12px] leading-5 text-red-500/80">
                {run.error}
              </p>
            ) : null}
            <RunGraph
              run={run}
              view={view}
              selectedKey={selected}
              onSelect={setPicked}
            />
            <RunNodeDetails
              run={run}
              nodeRunKey={selected}
              onOpenSession={openSession}
            />
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
