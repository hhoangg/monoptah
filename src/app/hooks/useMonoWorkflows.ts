import { useEffect, useRef } from "react";
import { listMonos } from "../../features/monos/model/mono";
import type {
  WorkflowId,
  WorkflowRun,
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
  setWorkflowRunning,
  subscribeCheckWorkflowsNow,
  takeWorkflowRequests,
  type ResumeRequest,
  updateRuns,
  updateWorkflows,
  workflowRunningId,
} from "../../features/workflows/model/workflowStore";
import { liveRunRegistry } from "./workflowLiveRuns";
import { executeRun, type MonoWorkflowHost } from "./workflowRunner";

export type { MonoWorkflowHost } from "./workflowRunner";

const TICK_MS = 60_000;

/**
 * Only the newest scheduler runs workflows, for the same reason as habits: a hot reload can
 * leave an older copy of this hook running beside the new one.
 */
const OWNER = "__monoWorkflowsScheduler";
type OwnedWindow = Window & { [OWNER]?: symbol };

/**
 * Asks for an interrupted run to continue from where it stopped. Only one window drives
 * workflows at a time, so the request is written to the workflow's file and the driver picks
 * it up on its next look (at most a minute; at once when this is the driver's window).
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

/** Whether a run of the workflow is going in this window or, by the registry, any other. */
const isRunning = (workflowId: string) =>
  workflowRunningId(workflowId) !== undefined ||
  liveRunRegistry()?.freshRunOf(workflowId) !== undefined;

/**
 * Runs each Mono's workflows while the app is open: on their schedule, when asked to run, and
 * when an interrupted run is resumed. Runs of different workflows go on side by side; one
 * workflow never has two runs at once. Pass `enabled` only for the one window that drives
 * workflows (see `workflowDriver`): a disabled hook never ticks, sweeps, resumes or claims.
 * Runs this window already has going do not depend on it: each looks after itself (see
 * `executeRun`), and the checks here leave any run a window is still driving alone.
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
    let swept = false;
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
      void executeRun(getHost, monoId, monoSessionId, run).catch((error) =>
        console.error(`Workflow "${run.definition.name}" could not run`, error),
      );
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
      // Refused while any window is driving a run of the workflow, this one's or another's.
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
     * Requests other windows left in the workflow files. Each resume is taken and cleared in
     * one hash-checked save, so only the driver that took it resumes the run.
     */
    const applySignals = async (monoId: string) => {
      for (const request of await takeWorkflowRequests(monoId, isRunning))
        await resume(request).catch((error) =>
          console.warn("Could not resume the workflow run", error),
        );
    };

    /** Runs a previous launch left running cannot be going on: mark them interrupted. */
    const sweepInterrupted = async () => {
      for (const { id } of listMonos()) {
        try {
          for (const workflow of await loadWorkflows(id)) {
            const runs = await loadRuns(id, workflow.id);
            if (!runs.some((entry) => entry.status === "running")) continue;
            // A run some window is still driving is not abandoned, whoever drives now.
            const abandoned = (entry: WorkflowRun) =>
              entry.status === "running" &&
              workflowRunningId(workflow.id) !== entry.id &&
              !liveRunRegistry()?.isFresh(entry.id);
            await updateRuns(id, workflow.id, (list) => ({
              runs: list.map((entry) =>
                abandoned(entry) ? markInterrupted(entry) : entry,
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
        // On taking over from a window that is gone, its unfinished runs are left running in
        // the files; mark them so they can be resumed.
        if (!swept) {
          swept = true;
          await sweepInterrupted();
        }
        for (const { id, sessionId } of listMonos()) {
          if (stopped) return;
          let workflows = await loadWorkflows(id);
          // A stop for a run that is going is left for the window driving it.
          if (
            workflows.some(
              (w) => w.resumeRunId || (w.stopRequested && !isRunning(w.id)),
            )
          ) {
            await applySignals(id);
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
    const stopChecks = subscribeCheckWorkflowsNow(() => void tick());
    return () => {
      stopped = true;
      window.clearInterval(timer);
      stopChecks();
      if (surface[OWNER] === owner) delete surface[OWNER];
    };
  }, [enabled]);
}
