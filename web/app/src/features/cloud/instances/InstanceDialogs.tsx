import { useLinkMutation } from "@/data";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { costLabel, INSTANCE_BUSY_MESSAGE, kindLabel } from "./copy";
import type { CloudInstance } from "./useCloudInstances";

interface DialogProps {
  instance: CloudInstance;
  onOpenChange: (open: boolean) => void;
}

/** The desktop refusal as plain text; `instance_busy` gets its own explanation. */
function errorText(error: { code?: string | undefined; message: string }, fallback: string): string {
  if (error.code === "instance_busy") return INSTANCE_BUSY_MESSAGE;
  return error.message.trim() ? error.message : fallback;
}

export interface StopDialogProps extends DialogProps {
  /** Called with the desktop `warning` (names interrupted busy sessions) after a successful request. */
  onStopped: (warning: string | undefined) => void;
}

/** Stop (P1): names the sessions on the instance and flags the busy ones. No cost notice: stopping lowers the bill. */
export function StopInstanceDialog({ instance, onOpenChange, onStopped }: StopDialogProps) {
  const stop = useLinkMutation("cloud.stop");
  const busy = instance.sessions.filter((session) => session.busy);
  const names = instance.sessions.map((session) => `"${session.title}"${session.busy ? " (busy, will be interrupted)" : ""}`);
  const shared = instance.sessions.length > 1;
  const description =
    instance.sessions.length === 0
      ? `${instance.displayName} has no sessions. Stopping it ends the instance's run.`
      : `${instance.displayName} runs ${shared ? `${instance.sessions.length} sessions that share it` : "1 session"}: ${names.join(", ")}.${
          busy.length > 0 ? ` ${busy.length === 1 ? "A busy session is" : `${busy.length} busy sessions are`} interrupted.` : ""
        }`;
  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title={`Stop ${kindLabel(instance.kind).toLowerCase()}?`}
      description={description}
      confirmLabel="Stop instance"
      destructive
      busy={stop.isPending}
      error={stop.isError ? errorText(stop.error, "Could not stop the instance.") : null}
      onConfirm={() =>
        stop.mutate(
          { kind: instance.kind, instance: instance.instance },
          {
            onSuccess: (result) => {
              onStopped(result.warning);
              onOpenChange(false);
            },
          },
        )
      }
    />
  );
}

export interface ResumeDialogProps extends DialogProps {
  /** The first session of the instance: `cloud.resume` takes a session. */
  sessionId: string;
  onResumed: () => void;
}

/** Resume (P2): starts the instance, which bills while it runs, so it needs an explicit confirm. */
export function ResumeInstanceDialog({ instance, sessionId, onOpenChange, onResumed }: ResumeDialogProps) {
  const resume = useLinkMutation("cloud.resume");
  const cost = instance.costEstimatePerHour !== undefined ? ` It costs ${costLabel(instance.costEstimatePerHour)} while it runs.` : "";
  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title="Resume this instance?"
      description={`Resuming starts ${instance.displayName} and it bills while it runs.${cost}`}
      confirmLabel="Resume and start billing"
      busy={resume.isPending}
      error={resume.isError ? errorText(resume.error, "Could not resume the instance.") : resume.data && !resume.data.accepted ? "The desktop did not start the instance." : null}
      onConfirm={() =>
        resume.mutate(
          { sessionId },
          {
            onSuccess: (result) => {
              if (!result.accepted) return;
              onResumed();
              onOpenChange(false);
            },
          },
        )
      }
    />
  );
}
