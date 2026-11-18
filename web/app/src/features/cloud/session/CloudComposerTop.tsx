// W-08 and U-16: everything the cloud adds above the composer of a session: startup progress, a failed
// start with Retry and Stop, the stopped and waking notes, the auto-stop notice with Keep running, the
// move progress, and "Not connected". State comes from `useCloud` with the catalog card as fallback.
// Desktop log text is never shown: only the sanitized `startup.error` and the desktop's refusal message,
// as plain text. Every control needs link permission `control`; other devices see it disabled with a note.
import { useState } from "react";
import type { CloudField, CloudState, SessionCard } from "@monoptah/protocol";
import { useCatalog, useCloud, useLinkMutation } from "@/data";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Notice } from "@/ui/Notice";
import { useCapability } from "@/version";
import { COST_NOTICES, KEPT_NOTE, NOT_CONNECTED_NOTE, READ_ONLY_NOTE, STOP_CONFIRM, STOPPED_NOTE, WAKING_NOTE } from "../copy";
import { isBillableKind, kindLabel, placementOf, useCanControl } from "../model";
import { StepList, moveItems, startupItems, startupItemsFromCard, type StepItem } from "../StepList";
import { useLiveCloudState } from "./liveState";

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

export function CloudComposerTop({ sessionId }: CloudComposerTopProps) {
  const enabled = useCapability("cloud");
  const cloud = useCloud(sessionId);
  const card = useCatalog().data?.sessions.find((candidate) => candidate.id === sessionId);
  const canControl = useCanControl();
  const placement = placementOf(card, cloud);
  const state: CloudState | undefined = placement?.state;
  useLiveCloudState(sessionId, state);

  // A stopped to starting change that this device did not start (Retry) is a send that woke the instance.
  // Derived during render from the previous state, not in an effect.
  const [track, setTrack] = useState<{ state: CloudState | undefined; waking: boolean }>({ state, waking: false });
  const [ownStart, setOwnStart] = useState(false);
  if (track.state !== state) {
    setTrack({ state, waking: state === "starting" && track.state === "stopped" && !ownStart });
    if (state !== "starting" && ownStart) setOwnStart(false);
  }
  const waking = track.waking;

  const [confirm, setConfirm] = useState<"retry" | "stop" | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const retry = useLinkMutation("cloud.retryStart");
  const stop = useLinkMutation("cloud.stop");
  const keep = useLinkMutation("cloud.keepRunning");

  if (!enabled || !placement || (!isBillableKind(placement.kind) && placement.kind !== "remote")) return null;

  const billable = isBillableKind(placement.kind);
  const startup = startupStepsOf(cloud, card);
  const failed = startup !== null && startup.error !== undefined;
  const showStartup = startup !== null && (state === "starting" || failed);
  const move = cloud?.move;
  const autoStop = cloud?.autoStop;
  const readOnlyId = `cloud-readonly-${sessionId}`;
  const errorOf = (error: { message: string } | null) => (error ? error.message || "The desktop refused the request." : null);
  const closeConfirm = () => setConfirm(null);

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
    billable && waking && state === "starting" ? (
      <Notice key="waking" title="Waking">
        {WAKING_NOTE}
      </Notice>
    ) : null,
  ];

  const anything = blocks.some(Boolean) || showStartup || move || autoStop || (billable && cloud?.kept);
  if (!anything && warning === null) return null;

  return (
    <section aria-label="Cloud instance" data-testid="cloud-composer-top" className="mx-auto flex w-full max-w-[720px] flex-col gap-2 px-3 pb-2">
      {blocks}
      {showStartup ? (
        <div className="rounded-[16px] border border-stroke bg-content-a04 p-3.5">
          <p className="mb-1 text-small font-semibold">{failed ? "Startup failed" : "Starting the instance"}</p>
          <StepList steps={startup.steps} title="Startup steps" />
          {failed && startup.error ? (
            <p role="alert" className="mt-2 text-small whitespace-pre-wrap text-deletions">
              {startup.error}
            </p>
          ) : null}
          {failed && billable ? (
            <div className="mt-3 flex flex-wrap gap-2">
              <Button type="button" size="sm" disabled={!canControl || retry.isPending} aria-describedby={canControl ? undefined : readOnlyId} onClick={() => setConfirm("retry")}>
                Retry
              </Button>
              {cloud?.kept ? (
                <Button type="button" size="sm" variant="outline" disabled={!canControl || stop.isPending} aria-describedby={canControl ? undefined : readOnlyId} onClick={() => setConfirm("stop")}>
                  Stop instance
                </Button>
              ) : null}
            </div>
          ) : null}
          {failed && billable && cloud?.kept ? <p className="mt-2 text-small text-muted-text">{KEPT_NOTE}</p> : null}
        </div>
      ) : null}
      {!showStartup && billable && cloud?.kept ? (
        <Notice title="Instance kept">
          {KEPT_NOTE}
          <Button type="button" size="sm" variant="outline" className="mt-2" disabled={!canControl || stop.isPending} onClick={() => setConfirm("stop")}>
            Stop instance
          </Button>
        </Notice>
      ) : null}
      {autoStop ? (
        <Notice title="Auto-stop announced">
          <p>
            The instance stops at {new Date(autoStop.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}. Reason: {autoStop.reason}
          </p>
          <Button type="button" size="sm" variant="outline" className="mt-2" disabled={!canControl || keep.isPending} aria-describedby={canControl ? undefined : readOnlyId} onClick={() => keep.mutate({ sessionId })}>
            Keep running
          </Button>
          {keep.isError ? <p className="mt-1 text-small text-deletions">{errorOf(keep.error)}</p> : null}
        </Notice>
      ) : null}
      {move ? (
        <div className="rounded-[16px] border border-stroke bg-content-a04 p-3.5">
          <p className="mb-1 text-small font-semibold">Moving the session</p>
          <StepList steps={moveItems(move)} title="Move steps" />
        </div>
      ) : null}
      {warning !== null ? <Notice title="Stop requested">{warning || "The instance is stopping."}</Notice> : null}
      {!canControl && (billable || autoStop) ? (
        <p id={readOnlyId} className="text-meta text-muted-text">
          {READ_ONLY_NOTE}
        </p>
      ) : null}
      <ConfirmDialog
        open={confirm === "retry"}
        onOpenChange={(open) => !open && closeConfirm()}
        title="Retry the start?"
        description={COST_NOTICES.retry}
        confirmLabel="Retry start"
        busy={retry.isPending}
        error={errorOf(retry.error)}
        onConfirm={() => {
          setOwnStart(true);
          retry.mutate({ sessionId }, { onSuccess: closeConfirm });
        }}
      />
      <ConfirmDialog
        open={confirm === "stop"}
        onOpenChange={(open) => !open && closeConfirm()}
        title="Stop this instance?"
        description={STOP_CONFIRM}
        confirmLabel="Stop instance"
        destructive
        busy={stop.isPending}
        error={errorOf(stop.error)}
        onConfirm={() => stop.mutate({ sessionId }, { onSuccess: (result) => { setWarning(result.warning ?? ""); closeConfirm(); } })}
      />
    </section>
  );
}
