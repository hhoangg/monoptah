import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import { useCatalog } from "@/data";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { safeMessage, safeName } from "../safe";
import { costLabel, kindLabel, lastUsedLabel, READ_ONLY_NOTE, STATE_LABELS, STOP_REPORT_LABELS } from "./copy";
import { ResumeInstanceDialog, StopInstanceDialog } from "./InstanceDialogs";
import type { CloudInstance } from "./useCloudInstances";

/** Stop applies to a run that is going or may be going: a stopped or not connected instance has nothing to stop. */
export function canStop(instance: CloudInstance): boolean {
  if (instance.state === "available" || instance.state === "starting" || instance.state === "unavailable") return true;
  return instance.state === "stopping" && instance.stopReport === "stillRunning";
}

export function canResume(instance: CloudInstance): boolean {
  return instance.state === "stopped" && instance.sessions.length > 0;
}

/**
 * The session `cloud.resume` names: one that is not busy and that the catalog still lists, else any that is not
 * busy, else the first. `cloud.resume` takes a session, and the desktop wakes the instance the session runs on.
 */
export function pickResumeSession(instance: CloudInstance, catalogIds: ReadonlySet<string> | undefined): string | undefined {
  const idle = instance.sessions.filter((session) => !session.busy);
  const listed = catalogIds ? idle.filter((session) => catalogIds.has(session.sessionId)) : [];
  return (listed[0] ?? idle[0] ?? instance.sessions[0])?.sessionId;
}

export interface InstanceRowProps {
  desktopId: string;
  instance: CloudInstance;
  readOnly: boolean;
  /** This device's own Stop or Resume was sent and the list has not shown its effect yet: the row's actions wait. */
  pending?: boolean;
  /** The `warning` the desktop returned for the last stop of this instance. */
  warning?: string | undefined;
  onStopped: (instance: CloudInstance, warning: string | undefined) => void;
  onResumed: (instance: CloudInstance) => void;
  nowMs: number;
}

export function InstanceRow({ desktopId, instance, readOnly, pending = false, warning, onStopped, onResumed, nowMs }: InstanceRowProps) {
  const [dialog, setDialog] = useState<"stop" | "resume" | null>(null);
  // The session of a Resume is chosen when the dialog opens, so a refetch cannot change it under the user.
  const [resumeSession, setResumeSession] = useState<string | undefined>(undefined);
  const catalog = useCatalog().data?.sessions;
  const stop = canStop(instance);
  const resume = canResume(instance);
  const noteId = useId();
  const name = safeName(instance.displayName);
  const disabled = readOnly || pending;
  return (
    <li data-testid="cloud-instance" data-instance={instance.instance} data-state={instance.state} className="flex flex-col gap-3 rounded-card bg-surface p-4">
      <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-2">
        <div className="min-w-0">
          <h2 className="wrap-anywhere text-body font-semibold text-content">{name}</h2>
          <p className="text-meta text-muted-text">
            {kindLabel(instance.kind)} - {lastUsedLabel(instance.lastUsedAt, nowMs)}
          </p>
        </div>
        <Badge variant="outline" role="status" aria-label={`State: ${STATE_LABELS[instance.state]}`} className="min-h-6 px-2">
          {STATE_LABELS[instance.state]}
        </Badge>
      </div>

      {instance.costEstimatePerHour !== undefined ? <p className="text-small text-muted-text">Cost: {costLabel(instance.costEstimatePerHour)}</p> : null}

      {instance.stopReport ? (
        <p role="status" data-testid="stop-report" data-report={instance.stopReport} className="flex items-center gap-2 text-small text-content">
          <Icon name={instance.stopReport === "stopping" ? "spinner" : "alert"} size={16} className="shrink-0" />
          {STOP_REPORT_LABELS[instance.stopReport]}
        </p>
      ) : null}

      {warning ? <Notice variant="info">{safeMessage(warning)}</Notice> : null}

      <div>
        <h3 className="text-label font-medium text-muted-text">Sessions</h3>
        {instance.sessions.length === 0 ? (
          <p className="text-small text-muted-text">No sessions use this instance.</p>
        ) : (
          <ul className="mt-1 flex flex-col">
            {instance.sessions.map((session) => (
              <li key={session.sessionId} className="flex min-h-11 items-center justify-between gap-2">
                <Link to="/d/$desktopId/s/$sessionId" params={{ desktopId, sessionId: session.sessionId }} className="min-w-0 flex-1 truncate text-small text-brand-text hover:underline">
                  {safeName(session.title)}
                </Link>
                {session.busy ? <Badge variant="outline">Busy</Badge> : null}
              </li>
            ))}
          </ul>
        )}
      </div>

      {stop || resume || pending ? (
        <div className="flex flex-wrap items-center gap-2">
          {stop ? (
            <Button type="button" variant="outline" size="sm" disabled={disabled} aria-describedby={disabled ? noteId : undefined} onClick={() => setDialog("stop")}>
              Stop
              <span className="sr-only"> {name}</span>
            </Button>
          ) : null}
          {resume ? (
            <Button
              type="button"
              size="sm"
              disabled={disabled}
              aria-describedby={disabled ? noteId : undefined}
              onClick={() => {
                setResumeSession(pickResumeSession(instance, catalog ? new Set(catalog.map((session) => session.id)) : undefined));
                setDialog("resume");
              }}
            >
              Resume
              <span className="sr-only"> {name}</span>
            </Button>
          ) : null}
          {readOnly ? (
            <p id={noteId} className="text-meta text-muted-text">
              {READ_ONLY_NOTE}
            </p>
          ) : pending ? (
            <p id={noteId} role="status" data-testid="own-action-note" className="text-meta text-muted-text">
              Waiting for the desktop to report the change.
            </p>
          ) : null}
        </div>
      ) : null}

      {dialog === "stop" ? (
        <StopInstanceDialog
          instance={instance}
          onOpenChange={(open) => !open && setDialog(null)}
          onStopped={(next) => onStopped(instance, next)}
        />
      ) : null}
      {dialog === "resume" && resumeSession ? (
        <ResumeInstanceDialog instance={instance} sessionId={resumeSession} onOpenChange={(open) => !open && setDialog(null)} onResumed={() => onResumed(instance)} />
      ) : null}
    </li>
  );
}
