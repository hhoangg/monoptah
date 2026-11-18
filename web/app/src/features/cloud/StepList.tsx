// The step list of the cloud UI: the 9 startup steps and the 7 move steps share it. Each row says its
// status in words and with an icon (never colour only), and the current step is announced politely.
import { useEffect, useState } from "react";
import type { CloudField, MoveStepId, StartupStepId, StepStatus } from "@monoptah/protocol";
import { MOVE_STEP_IDS, STARTUP_STEP_IDS } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { Icon, type IconName } from "@/ui/Icon";
import { MOVE_STEP_LABELS, STARTUP_STEP_LABELS, STEP_STATUS_LABELS } from "./model";

export interface StepItem {
  id: string;
  label: string;
  status: StepStatus;
  /** Epoch milliseconds from the desktop; both set means the step has a duration (X-10). */
  startedAt?: number | null | undefined;
  finishedAt?: number | null | undefined;
  /** The step the desktop is on now: highlighted, and `aria-current="step"`. */
  current?: boolean;
}

/** "8 s", "1 min 05 s". Never negative. */
export function durationLabel(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds} s`;
  return `${Math.floor(seconds / 60)} min ${String(seconds % 60).padStart(2, "0")} s`;
}

/** The time a step took (finished) or has taken so far (running), or null when the desktop sent no times. */
export function stepDuration(step: StepItem, nowMs: number): string | null {
  if (typeof step.startedAt !== "number") return null;
  if (step.status === "done" || step.status === "failed") return typeof step.finishedAt === "number" ? durationLabel(step.finishedAt - step.startedAt) : null;
  if (step.status === "running") return durationLabel(nowMs - step.startedAt);
  return null;
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
  // A running step shows how long it has run, so the list ticks once a second while one runs.
  const ticking = steps.some((step) => step.status === "running" && typeof step.startedAt === "number");
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setNowMs(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  return (
    <div className={className}>
      <p className="sr-only" role="status" aria-live="polite">
        {currentStepText(steps)}
      </p>
      <ol aria-label={title} className="flex flex-col gap-0.5">
        {steps.map((step) => (
          <li
            key={step.id}
            data-status={step.status}
            aria-current={step.current ? "step" : undefined}
            className={cn("flex min-h-7 items-center gap-2 rounded-row px-1.5 text-small", step.current && "bg-selection font-medium")}
          >
            <Icon
              name={STATUS_ICONS[step.status]}
              size={16}
              className={cn("shrink-0", step.status === "failed" ? "text-deletions" : step.status === "done" ? "text-content" : "text-muted-text", step.status === "running" && "motion-safe:animate-spin")}
            />
            <span className={cn("min-w-0 flex-1", step.status === "pending" || step.status === "skipped" ? "text-muted-text" : "text-content")}>{step.label}</span>
            <span className="shrink-0 text-meta text-muted-text">
              {STEP_STATUS_LABELS[step.status]}
              {stepDuration(step, nowMs) ? ` - ${stepDuration(step, nowMs)}` : ""}
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

/** The 9 startup steps from a `cloud.startup` field, always in the protocol order (missing steps are pending). */
export function startupItems(startup: NonNullable<CloudField["startup"]>): StepItem[] {
  const items: StepItem[] = STARTUP_STEP_IDS.map((id: StartupStepId) => {
    const step = startup.steps.find((candidate) => candidate.id === id);
    return { id, label: STARTUP_STEP_LABELS[id], status: step?.status ?? "pending", startedAt: step?.startedAt, finishedAt: step?.finishedAt };
  });
  // The desktop names the current step; without it the running or failed one is current.
  const currentId = startup.current ?? items.find((item) => item.status === "running" || item.status === "failed")?.id;
  return items.map((item) => (item.id === currentId ? { ...item, current: true } : item));
}

/** The 9 startup steps rebuilt from the catalog card's `startup.step` (before a watch frame arrives). */
export function startupItemsFromCard(step: StartupStepId, failed: boolean): StepItem[] {
  const at = STARTUP_STEP_IDS.indexOf(step);
  return STARTUP_STEP_IDS.map((id: StartupStepId, index) => ({
    id,
    label: STARTUP_STEP_LABELS[id],
    status: index < at ? "done" : index === at ? (failed ? "failed" : "running") : "pending",
    ...(index === at ? { current: true } : {}),
  }));
}

/** The 7 move steps from `cloud.move`: earlier steps are done, the current one has the reported status. */
export function moveItems(move: NonNullable<CloudField["move"]>): StepItem[] {
  const at = MOVE_STEP_IDS.indexOf(move.step);
  return MOVE_STEP_IDS.map((id: MoveStepId, index) => ({
    id,
    label: MOVE_STEP_LABELS[id],
    status: index < at ? "done" : index === at ? move.status : "pending",
    ...(index === at ? { current: true } : {}),
  }));
}
