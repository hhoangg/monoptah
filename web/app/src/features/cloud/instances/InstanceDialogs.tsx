// The Stop and Resume confirms of the instance list (U-17), also used by the session's own Stop. Names and
// messages come from the desktop and are untrusted: capped and stripped of hidden characters (`safe.ts`).
// The confirm is off, with a note, when this device loses control while a dialog is open.
import { useLinkMutation } from "@/data";
import { CloudConfirm } from "../CloudConfirm";
import { READ_ONLY_NOTE } from "../copy";
import { kindLabel, useCanControl } from "../model";
import { safeMessage, safeName } from "../safe";
import { costLabel, INSTANCE_BUSY_MESSAGE } from "./copy";
import type { CloudInstance } from "./useCloudInstances";

interface DialogProps {
  instance: CloudInstance;
  onOpenChange: (open: boolean) => void;
}

/** The desktop refusal as plain text; `instance_busy` gets its own explanation. */
function errorText(error: { code?: string | undefined; message: string }, fallback: string): string {
  if (error.code === "instance_busy") return INSTANCE_BUSY_MESSAGE;
  return safeMessage(error.message, fallback);
}

export interface StopDialogProps extends DialogProps {
  /** Called with the desktop `warning` (names interrupted busy sessions) after a successful request. */
  onStopped: (warning: string | undefined) => void;
}

/** Stop (P1): names the sessions on the instance and flags the busy ones. No cost notice: stopping lowers the bill. */
export function StopInstanceDialog({ instance, onOpenChange, onStopped }: StopDialogProps) {
  const canControl = useCanControl();
  const stop = useLinkMutation("cloud.stop");
  const name = safeName(instance.displayName);
  const busy = instance.sessions.filter((session) => session.busy);
  const names = instance.sessions.map((session) => `"${safeName(session.title)}"${session.busy ? " (busy, will be interrupted)" : ""}`);
  const shared = instance.sessions.length > 1;
  const description =
    instance.sessions.length === 0
      ? `${name} has no sessions. Stopping it ends the instance's run.`
      : `${name} runs ${shared ? `${instance.sessions.length} sessions that share it` : "1 session"}: ${names.join(", ")}.${
          busy.length > 0 ? ` ${busy.length === 1 ? "A busy session is" : `${busy.length} busy sessions are`} interrupted.` : ""
        }`;
  return (
    <CloudConfirm
      open
      onOpenChange={onOpenChange}
      title={`Stop ${kindLabel(instance.kind)}?`}
      description={description}
      confirmLabel="Stop instance"
      destructive
      busy={stop.isPending}
      confirmDisabled={!canControl}
      notice={canControl ? null : READ_ONLY_NOTE}
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
  /** The session `cloud.resume` names (chosen by the row, see `pickResumeSession`). */
  sessionId: string;
  onResumed: () => void;
}

/** Resume (P2): starts the instance, which bills while it runs, so it needs an explicit confirm. */
export function ResumeInstanceDialog({ instance, sessionId, onOpenChange, onResumed }: ResumeDialogProps) {
  const canControl = useCanControl();
  const resume = useLinkMutation("cloud.resume");
  const cost = instance.costEstimatePerHour !== undefined ? ` It costs ${costLabel(instance.costEstimatePerHour)} while it runs.` : "";
  return (
    <CloudConfirm
      open
      onOpenChange={onOpenChange}
      title="Resume this instance?"
      description={`Resuming starts ${safeName(instance.displayName)} and it bills while it runs.${cost}`}
      confirmLabel="Resume and start billing"
      busy={resume.isPending}
      confirmDisabled={!canControl}
      notice={canControl ? null : READ_ONLY_NOTE}
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
