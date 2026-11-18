// The step list of the cloud UI: the 9 startup steps and the 7 move steps share it. Each row says its
// status in words and with an icon (never colour only), and the current step is announced politely.
import type { CloudField, MoveStepId, StartupStepId, StepStatus } from "@monoptah/protocol";
import { MOVE_STEP_IDS, STARTUP_STEP_IDS } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { Icon, type IconName } from "@/ui/Icon";
import { MOVE_STEP_LABELS, STARTUP_STEP_LABELS, STEP_STATUS_LABELS } from "./model";

export interface StepItem {
  id: string;
  label: string;
  status: StepStatus;
}

const STATUS_ICONS: Readonly<Record<StepStatus, IconName>> = {
  pending: "circle",
  running: "spinner",
  done: "check",
  failed: "alert",
  skipped: "minus",
};

/** The announcement for the live region: the step that runs or failed, else nothing. */
export function currentStepText(steps: readonly StepItem[]): string {
  const index = steps.findIndex((step) => step.status === "running" || step.status === "failed");
  const step = steps[index];
  if (!step) return "";
  return `Step ${index + 1} of ${steps.length}: ${step.label}, ${STEP_STATUS_LABELS[step.status].toLowerCase()}`;
}

export function StepList({ steps, title, className }: { steps: readonly StepItem[]; title: string; className?: string }) {
  return (
    <div className={className}>
      <p className="sr-only" role="status" aria-live="polite">
        {currentStepText(steps)}
      </p>
      <ol aria-label={title} className="flex flex-col gap-0.5">
        {steps.map((step) => (
          <li key={step.id} data-status={step.status} className="flex min-h-7 items-center gap-2 text-small">
            <Icon
              name={STATUS_ICONS[step.status]}
              size={16}
              className={cn("shrink-0", step.status === "failed" ? "text-deletions" : step.status === "done" ? "text-content" : "text-muted-text", step.status === "running" && "motion-safe:animate-spin")}
            />
            <span className={cn("min-w-0 flex-1", step.status === "pending" || step.status === "skipped" ? "text-muted-text" : "text-content")}>{step.label}</span>
            <span className="shrink-0 text-meta text-muted-text">{STEP_STATUS_LABELS[step.status]}</span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/** The 9 startup steps from a `cloud.startup` field, always in the protocol order (missing steps are pending). */
export function startupItems(startup: NonNullable<CloudField["startup"]>): StepItem[] {
  return STARTUP_STEP_IDS.map((id: StartupStepId) => ({
    id,
    label: STARTUP_STEP_LABELS[id],
    status: startup.steps.find((step) => step.id === id)?.status ?? "pending",
  }));
}

/** The 9 startup steps rebuilt from the catalog card's `startup.step` (before a watch frame arrives). */
export function startupItemsFromCard(step: StartupStepId, failed: boolean): StepItem[] {
  const at = STARTUP_STEP_IDS.indexOf(step);
  return STARTUP_STEP_IDS.map((id: StartupStepId, index) => ({
    id,
    label: STARTUP_STEP_LABELS[id],
    status: index < at ? "done" : index === at ? (failed ? "failed" : "running") : "pending",
  }));
}

/** The 7 move steps from `cloud.move`: earlier steps are done, the current one has the reported status. */
export function moveItems(move: NonNullable<CloudField["move"]>): StepItem[] {
  const at = MOVE_STEP_IDS.indexOf(move.step);
  return MOVE_STEP_IDS.map((id: MoveStepId, index) => ({
    id,
    label: MOVE_STEP_LABELS[id],
    status: index < at ? "done" : index === at ? move.status : "pending",
  }));
}
