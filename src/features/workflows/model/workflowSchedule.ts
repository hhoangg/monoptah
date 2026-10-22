import {
  MISSED_RUN_GRACE_MS,
  nextHabitRunAt,
} from "../../monos/model/monoHabits";
import { triggerOf, type Workflow } from "./workflow";

/** The schedule a workflow runs on, when its trigger is a schedule. */
function scheduleOf(workflow: Workflow) {
  const trigger = triggerOf(workflow);
  return trigger?.type === "schedule" ? trigger.schedule : undefined;
}

/**
 * Workflows to run now, and ones whose schedule only has to move on.
 * Like habits, one that came due long ago (the app was closed) is not run, and one that
 * comes due while its previous run is going is skipped: it runs again at its next time.
 * A requested run waits for the current one to end instead of being dropped.
 */
export function dueWorkflows(
  workflows: Workflow[],
  now: number,
  isRunning: (workflowId: string) => boolean,
): { run: Workflow[]; advance: Workflow[] } {
  const run: Workflow[] = [];
  const advance: Workflow[] = [];
  for (const workflow of workflows) {
    const busy = isRunning(workflow.id);
    if (workflow.runRequested) {
      if (!busy) run.push(workflow);
      continue;
    }
    if (!workflow.enabled || !scheduleOf(workflow)) continue;
    // Enabled with no next time yet: pick one, do not run.
    if (workflow.nextRunAt === undefined) {
      advance.push(workflow);
      continue;
    }
    if (workflow.nextRunAt > now) continue;
    if (!busy && now - workflow.nextRunAt <= MISSED_RUN_GRACE_MS)
      run.push(workflow);
    else advance.push(workflow);
  }
  return { run, advance };
}

/** The workflow with its schedule moved past `now` and its run request answered. */
export function afterClaim(
  workflow: Workflow,
  now: number,
  runId?: string,
): Workflow {
  // A stop asked for before this run started was meant for an earlier one.
  const { runRequested, stopRequested: _stop, ...rest } = workflow;
  const schedule = scheduleOf(workflow);
  const keep =
    runRequested && workflow.nextRunAt !== undefined && workflow.nextRunAt > now;
  return {
    ...rest,
    ...(schedule && !keep ? { nextRunAt: nextHabitRunAt(schedule, now) } : {}),
    ...(runId ? { lastRunId: runId } : {}),
  };
}
