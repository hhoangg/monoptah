import { useEffect, useRef } from "react";
import { listMonos } from "../../features/monos/model/mono";
import type {
  Workflow,
  WorkflowId,
} from "../../features/workflows/model/workflow";
import {
  markInterrupted,
  resumeRun,
  startRun,
} from "../../features/workflows/model/workflowEngine";
import { validateWorkflow } from "../../features/workflows/model/workflowParse";
import {
  afterClaim,
  dueWorkflows,
} from "../../features/workflows/model/workflowSchedule";
import {
  checkWorkflowsNow,
  clearWorkflowStop,
  loadRuns,
  loadWorkflows,
  requestWorkflowStop,
  setWorkflowRunning,
  subscribeCheckWorkflowsNow,
  updateRuns,
  updateWorkflows,
  workflowRunningId,
} from "../../features/workflows/model/workflowStore";
import { executeRun, type MonoWorkflowHost } from "./workflowRunner";

export type { MonoWorkflowHost } from "./workflowRunner";

const TICK_MS = 60_000;

/**
 * Only the newest scheduler runs workflows, for the same reason as habits: a hot reload can
 * leave an older copy of this hook running beside the new one.
 */
const OWNER = "__monoWorkflowsScheduler";
/** Set once this window has marked the runs a previous launch left unfinished. */
const SWEPT = "__monoWorkflowsSwept";
type OwnedWindow = Window & { [OWNER]?: symbol; [SWEPT]?: boolean };

type ResumeRequest = { monoId: string; workflowId: WorkflowId; runId: string };

/** How soon a stop asked for in another window reaches a run that is going. */
const STOP_CHECK_MS = 5_000;

/**
 * Asks for an interrupted run to continue from where it stopped. Only the main window drives
 * runs, so the request is written to the workflow's file and the driver picks it up on its
 * next look (at most a minute; at once when this is the driver's window).
 */
export async function resumeWorkflowRun(
  monoId: string,
  workflowId: WorkflowId,
  runId: string,
): Promise<void> {
  await updateWorkflows(monoId, (list) => ({
    workflows: list.map((w) =>
      w.id === workflowId ? { ...w, resumeRunId: runId } : w,
    ),
    result: undefined,
  }));
  checkWorkflowsNow();
}

const isRunning = (workflowId: string) =>
  workflowRunningId(workflowId) !== undefined;

/**
 * Runs each Mono's workflows while the app is open: on their schedule, when asked to run, and
 * when an interrupted run is resumed. Runs of different workflows go on side by side; one
 * workflow never has two runs at once. Pass `enabled` only for the one window that drives
 * workflows (see `workflowDriver`): a disabled hook never ticks, sweeps, resumes or claims.
 */
export function useMonoWorkflows(host: MonoWorkflowHost, enabled = true) {
  const hostRef = useRef(host);
  hostRef.current = host;

  useEffect(() => {
    if (!enabled) return;
    const owner = Symbol("mono-workflows");
    const surface = window as OwnedWindow;
    surface[OWNER] = owner;
    let stopped = false;
    let ticking = false;
    let watching = false;
    let liveRuns = 0;
    // A check asked for mid-tick happens as soon as that tick ends.
    let checkAgain = false;
    const getHost = () => hostRef.current;

    const launch = (
      monoId: string,
      monoSessionId: string,
      run: Parameters<typeof executeRun>[3],
    ) => {
      setWorkflowRunning(run.workflowId, run.id);
      // A request made before this run started was meant for an earlier one.
      clearWorkflowStop(run.workflowId);
      liveRuns++;
      void executeRun(getHost, monoId, monoSessionId, run)
        .catch((error) =>
          console.error(
            `Workflow "${run.definition.name}" could not run`,
            error,
          ),
        )
        .finally(() => liveRuns--);
    };

    const startWorkflow = async (
      monoId: string,
      monoSessionId: string,
      workflowId: WorkflowId,
    ) => {
      const now = Date.now();
      const runId = crypto.randomUUID();
      // Claim from the latest definition: it may have been paused, removed or claimed elsewhere.
      const claimed = await updateWorkflows(monoId, (list) => {
        const due = dueWorkflows(
          list.filter((entry) => entry.id === workflowId),
          now,
          isRunning,
        ).run[0];
        return {
          workflows: due
            ? list.map((entry) =>
                entry.id === workflowId ? afterClaim(entry, now, runId) : entry,
              )
            : list,
          result: due,
        };
      });
      if (!claimed || stopped) return;
      let run = startRun(
        claimed,
        claimed.runRequested ? "manual" : "schedule",
        now,
        runId,
      );
      const issues = validateWorkflow(claimed);
      if (issues.length) {
        const error = `The workflow is not valid: ${issues.map((issue) => issue.message).join("; ")}`;
        run = { ...run, status: "failed", endedAt: now, error };
        // A run that never starts would otherwise only show in the run history.
        try {
          getHost().post(
            monoSessionId,
            claimed,
            runId,
            `${claimed.name} · failed`,
            `## ${claimed.name} · failed\n\nThe workflow could not start. ${error}`,
          );
        } catch (postError) {
          console.error("Could not post the workflow failure", postError);
        }
      }
      launch(monoId, monoSessionId, run);
    };

    const resume = async ({ monoId, workflowId, runId }: ResumeRequest) => {
      const monoSessionId = listMonos().find(
        (mono) => mono.id === monoId,
      )?.sessionId;
      if (!monoSessionId) return;
      if (isRunning(workflowId)) {
        console.warn(`Workflow ${workflowId} is already running; not resumed`);
        return;
      }
      const stored = (await loadRuns(monoId, workflowId)).find(
        (entry) => entry.id === runId,
      );
      if (stored?.status !== "interrupted") return;
      launch(monoId, monoSessionId, resumeRun(stored));
    };

    /**
     * Requests other windows left in the workflow files. A stop reaches a run that is going; a
     * request for a run that is not (it ended first) is just cleared.
     */
    const applySignals = async (
      monoId: string,
      workflows: Workflow[],
      resumes: boolean,
    ) => {
      const stops = workflows.filter((w) => w.stopRequested);
      const resuming = resumes ? workflows.filter((w) => w.resumeRunId) : [];
      if (!stops.length && !resuming.length) return;
      for (const w of stops) if (isRunning(w.id)) requestWorkflowStop(w.id);
      const asked = new Map(resuming.map((w) => [w.id, w.resumeRunId]));
      await updateWorkflows(monoId, (list) => ({
        workflows: list.map((w) => {
          const { stopRequested: _stop, resumeRunId: _resume, ...rest } = w;
          const clearStop = stops.some((entry) => entry.id === w.id);
          const clearResume =
            asked.get(w.id) !== undefined && asked.get(w.id) === w.resumeRunId;
          return {
            ...rest,
            ...(w.stopRequested && !clearStop ? { stopRequested: true } : {}),
            ...(w.resumeRunId && !clearResume
              ? { resumeRunId: w.resumeRunId }
              : {}),
          };
        }),
        result: undefined,
      }));
      for (const w of resuming)
        await resume({
          monoId,
          workflowId: w.id,
          runId: w.resumeRunId as string,
        }).catch((error) =>
          console.warn("Could not resume the workflow run", error),
        );
    };

    /** While runs are going, looks often for a stop asked for in another window. */
    const watchStops = async () => {
      if (stopped || watching || liveRuns === 0) return;
      watching = true;
      try {
        for (const { id } of listMonos())
          await applySignals(id, await loadWorkflows(id), false);
      } catch (error) {
        console.warn("Could not check for workflow stop requests", error);
      } finally {
        watching = false;
      }
    };

    /** Runs a previous launch left running cannot be going on: mark them interrupted. */
    const sweepInterrupted = async () => {
      for (const { id } of listMonos()) {
        try {
          for (const workflow of await loadWorkflows(id)) {
            if (isRunning(workflow.id)) continue;
            const runs = await loadRuns(id, workflow.id);
            if (!runs.some((entry) => entry.status === "running")) continue;
            await updateRuns(id, workflow.id, (list) => ({
              runs: list.map((entry) =>
                entry.status === "running" && !isRunning(workflow.id)
                  ? markInterrupted(entry)
                  : entry,
              ),
              result: undefined,
            }));
          }
        } catch (error) {
          console.warn("Could not check for interrupted workflow runs", error);
        }
      }
    };

    const tick = async () => {
      if (surface[OWNER] !== owner) stopped = true;
      if (stopped) return;
      if (ticking) {
        checkAgain = true;
        return;
      }
      ticking = true;
      checkAgain = false;
      try {
        if (!surface[SWEPT]) {
          surface[SWEPT] = true;
          await sweepInterrupted();
        }
        for (const { id, sessionId } of listMonos()) {
          if (stopped) return;
          let workflows = await loadWorkflows(id);
          if (workflows.some((w) => w.stopRequested || w.resumeRunId)) {
            await applySignals(id, workflows, true);
            workflows = await loadWorkflows(id);
          }
          const now = Date.now();
          const { run, advance } = dueWorkflows(workflows, now, isRunning);
          if (advance.length) {
            for (const workflow of advance)
              if (isRunning(workflow.id) && workflow.nextRunAt !== undefined)
                console.info(
                  `Workflow "${workflow.name}" is still running; skipped this time`,
                );
            await updateWorkflows(id, (list) => {
              const stale = new Set(
                dueWorkflows(list, Date.now(), isRunning).advance.map(
                  (workflow) => workflow.id,
                ),
              );
              return {
                workflows: list.map((workflow) =>
                  stale.has(workflow.id)
                    ? afterClaim(workflow, Date.now())
                    : workflow,
                ),
                result: undefined,
              };
            });
          }
          // Workflows are set up in the Mono's chat, so a Mono never opened has none to report to.
          if (!sessionId) continue;
          for (const workflow of run) {
            if (stopped) return;
            await startWorkflow(id, sessionId, workflow.id).catch((error) =>
              console.warn(
                `Workflow "${workflow.name}" could not start`,
                error,
              ),
            );
          }
        }
      } catch (error) {
        console.warn("Could not check Mono workflows", error);
      } finally {
        ticking = false;
        if (checkAgain && !stopped) void tick();
      }
    };

    void tick();
    const timer = window.setInterval(() => void tick(), TICK_MS);
    const stopTimer = window.setInterval(
      () => void watchStops(),
      STOP_CHECK_MS,
    );
    const stopChecks = subscribeCheckWorkflowsNow(() => void tick());
    return () => {
      stopped = true;
      window.clearInterval(timer);
      window.clearInterval(stopTimer);
      stopChecks();
      if (surface[OWNER] === owner) delete surface[OWNER];
    };
  }, [enabled]);
}
