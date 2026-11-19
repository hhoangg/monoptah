// W-08 and U-16: everything the cloud adds above the composer of a session: startup progress, a failed
// start with Retry and Stop, the stopped and waking notes, the auto-stop notice with Keep running, the
// move progress, and "Not connected". State comes from `useCloud` with the catalog card as fallback.
// Desktop log text is never shown: only the sanitized `startup.error` and the desktop's refusal message,
// as plain text, capped and stripped of hidden characters (`safe.ts`). Every control needs link permission
// `control`; other devices see it disabled with a note.
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { CloudField, CloudState, SessionCard } from "@monoptah/protocol";
import { conversationQueryOptions, useCatalog, useCloud, useLinkMutation, useLinkQuery } from "@/data";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { useCapability } from "@/version";
import { CloudConfirm } from "../CloudConfirm";
import {
  COST_NOTICES,
  KEEP_RUNNING_NOTE,
  KEPT_NOTE,
  NOT_CONNECTED_NOTE,
  READ_ONLY_NOTE,
  STOP_CONFIRM,
  STOPPED_NOTE,
  STOPPING_NOTE,
  UNAVAILABLE_NOTE,
  WAKING_NOTE,
} from "../copy";
import { StopInstanceDialog } from "../instances/InstanceDialogs";
import { isBillableKind, kindLabel, placementOf, placementSources, useCanControl } from "../model";
import { safeMessage } from "../safe";
import { StepList, moveItems, startupItems, startupItemsFromCard, type StepItem } from "../StepList";
import { useLivePlacement } from "./liveState";

export interface CloudComposerTopProps {
  desktopId: string;
  sessionId: string;
}

/** The startup steps to show: from the live `cloud.startup`, else rebuilt from the catalog card. */
function startupStepsOf(cloud: CloudField | undefined, card: SessionCard | undefined): { steps: StepItem[]; error: string | undefined } | null {
  const live = cloud?.startup;
  if (live) {
    const steps = startupItems(live);
    return { steps, error: live.error ?? (steps.some((step) => step.status === "failed") ? "" : undefined) };
  }
  if (cloud) return null;
  const fromCard = card?.startup;
  if (!fromCard) return null;
  return { steps: startupItemsFromCard(fromCard.step, fromCard.error !== undefined), error: fromCard.error };
}

/**
 * The Stop confirm for the session's own instance. It names the sessions that share the instance (from
 * `cloud.instances`, a read that never wakes anything) by reusing the instance list's dialog. Only when the
 * list cannot be read does it fall back to the generic text.
 */
function SessionStopDialog({ cloud, sessionId, onClose, onStopped }: { cloud: CloudField | undefined; sessionId: string; onClose: () => void; onStopped: (warning: string | undefined) => void }) {
  const canControl = useCanControl();
  const list = useLinkQuery("cloud.instances", {});
  const stop = useLinkMutation("cloud.stop");
  const found = list.data?.instances.find((candidate) => candidate.kind === cloud?.kind && candidate.instance === cloud?.instance);
  // Once matched, the dialog stays the instance dialog even if a refetch drops the row: switching components
  // would unmount an in-flight `cloud.stop` and lose its warning.
  const [pinned, setPinned] = useState<typeof found>(undefined);
  if (found && !pinned) setPinned(found);
  const instance = pinned ?? found;
  if (instance) return <StopInstanceDialog instance={instance} onOpenChange={(open) => !open && onClose()} onStopped={onStopped} />;
  const loading = list.isPending;
  return (
    <CloudConfirm
      open
      onOpenChange={(open) => !open && onClose()}
      title="Stop this instance?"
      description={loading ? "Looking up the sessions that share this instance..." : STOP_CONFIRM}
      confirmLabel="Stop instance"
      destructive
      busy={stop.isPending}
      confirmDisabled={!canControl || loading}
      notice={canControl ? null : READ_ONLY_NOTE}
      error={stop.isError ? safeMessage(stop.error.message, "The desktop refused the request.") : null}
      onConfirm={() =>
        stop.mutate(
          { sessionId },
          {
            onSuccess: (result) => {
              onStopped(result.warning);
              onClose();
            },
          },
        )
      }
    />
  );
}

export function CloudComposerTop({ desktopId, sessionId }: CloudComposerTopProps) {
  const enabled = useCapability("cloud");
  const cloud = useCloud(sessionId);
  const catalog = useCatalog();
  const card = catalog.data?.sessions.find((candidate) => candidate.id === sessionId);
  const canControl = useCanControl();
  const placement = placementOf(card, cloud);
  const state: CloudState | undefined = placement?.state;
  useLivePlacement(desktopId, sessionId, placementSources(card, cloud, catalog.dataUpdatedAt > 0));

  // "Waking, your message is queued" only after THIS device sent: this device has a message in flight (sent or
  // sending, not yet in the transcript) while the instance starts. The send guard refuses a send to an instance
  // that is starting, so such a message was sent while it was asleep and woke it. A stopped to starting change
  // by the desktop user or another device leaves no in-flight message here, so it shows no such note.
  const sentHere =
    useQuery({ ...conversationQueryOptions(desktopId, sessionId), enabled: false, select: (conversation) => conversation.optimistic.some((turn) => turn.status !== "not_sent") }).data ?? false;
  const waking = sentHere && state === "starting";

  const [confirm, setConfirm] = useState<"retry" | "stop" | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const retry = useLinkMutation("cloud.retryStart");
  const keep = useLinkMutation("cloud.keepRunning");

  if (!enabled || !placement || (!isBillableKind(placement.kind) && placement.kind !== "remote")) return null;

  const billable = isBillableKind(placement.kind);
  const startup = startupStepsOf(cloud, card);
  const failed = startup !== null && startup.error !== undefined;
  const showStartup = startup !== null && (state === "starting" || failed);
  const move = cloud?.move;
  const autoStop = cloud?.autoStop;
  const readOnlyId = `cloud-readonly-${sessionId}`;
  const describedBy = canControl ? undefined : readOnlyId;
  const errorOf = (error: { message: string } | null) => (error ? safeMessage(error.message, "The desktop refused the request.") : null);
  const closeConfirm = () => setConfirm(null);
  const startupError = startup?.error ? safeMessage(startup.error) : "";

  const blocks = [
    state === "unknown" ? (
      <Notice key="unknown" title={`${kindLabel(placement.kind)}: Not connected`}>
        {NOT_CONNECTED_NOTE}
      </Notice>
    ) : null,
    billable && state === "stopped" ? (
      <Notice key="stopped" title="Stopped">
        {STOPPED_NOTE}
      </Notice>
    ) : null,
    billable && state === "stopping" ? (
      <Notice key="stopping" title="Stopping">
        {STOPPING_NOTE}
      </Notice>
    ) : null,
    billable && state === "unavailable" ? (
      <Notice key="unavailable" title="Unavailable">
        {UNAVAILABLE_NOTE}
      </Notice>
    ) : null,
    billable && waking ? (
      <Notice key="waking" title="Waking">
        {WAKING_NOTE}
      </Notice>
    ) : null,
  ];

  const anything = blocks.some(Boolean) || showStartup || move || autoStop || (billable && cloud?.kept);
  if (!anything && warning === null) return null;

  const retryButton = (
    <Button type="button" size="sm" disabled={!canControl || retry.isPending} aria-describedby={describedBy} onClick={() => setConfirm("retry")}>
      Retry
    </Button>
  );
  const stopButton = (
    <Button type="button" size="sm" variant="outline" disabled={!canControl} aria-describedby={describedBy} onClick={() => setConfirm("stop")}>
      Stop instance
    </Button>
  );

  return (
    <section aria-label="Cloud instance" data-testid="cloud-composer-top" className="mx-auto flex w-full max-w-[720px] flex-col gap-2 px-3 pb-2">
      {blocks}
      {showStartup ? (
        <div className="rounded-[16px] border border-stroke bg-content-a04 p-3.5">
          <p className="mb-1 text-small font-semibold">{failed ? "Startup failed" : "Starting the instance"}</p>
          <StepList steps={startup.steps} title="Startup steps" />
          {failed && startupError ? (
            <p role="alert" className="mt-2 text-small whitespace-pre-wrap text-deletions">
              {startupError}
            </p>
          ) : null}
          {failed && billable ? (
            <div className="mt-3 flex flex-wrap gap-2">
              {retryButton}
              {cloud?.kept ? stopButton : null}
            </div>
          ) : null}
          {failed && billable && cloud?.kept ? <p className="mt-2 text-small text-muted-text">{KEPT_NOTE}</p> : null}
        </div>
      ) : null}
      {!showStartup && billable && cloud?.kept ? (
        <Notice title="Instance kept">
          {KEPT_NOTE}
          <div className="mt-2 flex flex-wrap gap-2">
            {retryButton}
            {stopButton}
          </div>
        </Notice>
      ) : null}
      {autoStop ? (
        <Notice title="Auto-stop announced">
          <p>
            The instance stops at {new Date(autoStop.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}. Reason: {safeMessage(autoStop.reason, "No reason given")}
          </p>
          <Button type="button" size="sm" variant="outline" className="mt-2" disabled={!canControl || keep.isPending} aria-describedby={describedBy} onClick={() => keep.mutate({ sessionId })}>
            Keep running
          </Button>
          <p className="mt-1 text-meta text-muted-text">{KEEP_RUNNING_NOTE}</p>
          {keep.isError ? <p className="mt-1 text-small text-deletions">{errorOf(keep.error)}</p> : null}
        </Notice>
      ) : null}
      {move ? (
        <div className="rounded-[16px] border border-stroke bg-content-a04 p-3.5">
          <p className="mb-1 text-small font-semibold">Moving the session</p>
          <StepList steps={moveItems(move)} title="Move steps" />
        </div>
      ) : null}
      {warning !== null ? <Notice title="Stop requested">{safeMessage(warning) || "The instance is stopping."}</Notice> : null}
      {!canControl && (billable || autoStop) ? (
        <p id={readOnlyId} className="text-meta text-muted-text">
          {READ_ONLY_NOTE}
        </p>
      ) : null}
      <CloudConfirm
        open={confirm === "retry"}
        onOpenChange={(open) => !open && closeConfirm()}
        title="Retry the start?"
        description={COST_NOTICES.retry}
        confirmLabel="Retry start"
        busy={retry.isPending}
        confirmDisabled={!canControl}
        notice={canControl ? null : READ_ONLY_NOTE}
        error={errorOf(retry.error)}
        onConfirm={() => retry.mutate({ sessionId }, { onSuccess: closeConfirm })}
      />
      {confirm === "stop" ? (
        <SessionStopDialog
          cloud={cloud}
          sessionId={sessionId}
          onClose={closeConfirm}
          onStopped={(next) => setWarning(next ?? "")}
        />
      ) : null}
    </section>
  );
}
