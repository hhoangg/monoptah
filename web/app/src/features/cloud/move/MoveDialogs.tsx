// The dialogs behind the move menu items (U-19) and the host that mounts them. Each move needs a cost notice
// and an explicit confirm. Cancel move says the workspace is kept and never offers deleting anything. The
// progress of the 7 steps is the step list of `CloudComposerTop` (composer-top), not here.
//
// A dialog opened from a list card is rendered inside the card's `<Link>` in the React tree, so every click in
// it is contained (`ContainClicks`): Cancel must not navigate to the session. The permission and the
// `cloud` and `cloudMove` capabilities are re-checked while the dialog is open and again at confirm time.
import { useEffect, useId, useState } from "react";
import type { SessionCard, SessionStartTarget } from "@monoptah/protocol";
import { useLinkMutation, useLinkQuery } from "@/data";
import { useRefreshSessions } from "@/features/sessions/SessionActionDialogs";
import { Button } from "@/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Notice } from "@/ui/Notice";
import { useCapability } from "@/version";
import { CloudConfirm, ContainClicks } from "../CloudConfirm";
import { CANCEL_MOVE_CONFIRM, COST_NOTICES, MOVE_DISABLED_NOTE, READ_ONLY_NOTE } from "../copy";
import { useCanControl } from "../model";
import { safeMessage } from "../safe";
import { TargetControls } from "../start/TargetControls";
import { closeMove, moveHostKey, registerHost, useMoveSnapshot } from "./moveStore";

interface MoveDialogProps {
  desktopId: string;
  session: SessionCard;
}

/** True while this device may move sessions: control permission and both capabilities. */
export function useMoveAllowed(): { allowed: boolean; canControl: boolean } {
  const canControl = useCanControl();
  const cloud = useCapability("cloud");
  const cloudMove = useCapability("cloudMove");
  return { allowed: canControl && cloud && cloudMove, canControl };
}

/** Why a move is off: read-only first, else the desktop toggles. */
function offNote(canControl: boolean): string {
  return canControl ? MOVE_DISABLED_NOTE : READ_ONLY_NOTE;
}

/** The desktop refusal as plain text; `disabled` (the toggles are off) gets its own explanation. */
function errorText(error: { code?: string | undefined; message: string }, fallback: string): string {
  if (error.code === "disabled") return MOVE_DISABLED_NOTE;
  return safeMessage(error.message, fallback);
}

function onOpenChange(open: boolean) {
  if (!open) closeMove();
}

function MoveToCloudDialog({ desktopId, session }: MoveDialogProps) {
  const { allowed, canControl } = useMoveAllowed();
  const options = useLinkQuery("cloud.startOptions", { projectId: session.projectId }, { enabled: allowed });
  const move = useLinkMutation("cloud.moveToCloud");
  const refresh = useRefreshSessions(desktopId);
  const [target, setTarget] = useState<SessionStartTarget | null>(null);
  const busy = move.isPending;
  const submit = () => {
    if (!allowed) return;
    move.mutate(
      { sessionId: session.id, ...(target ? { target } : {}) },
      {
        onSuccess: () => {
          refresh();
          closeMove();
        },
      },
    );
  };
  return (
    <ContainClicks>
      <Dialog open onOpenChange={(open) => (busy ? undefined : onOpenChange(open))}>
        <DialogContent showClose={false}>
          <DialogHeader className="pr-0">
            <DialogTitle>Move this session to the cloud?</DialogTitle>
            <DialogDescription>{COST_NOTICES.moveToCloud}</DialogDescription>
          </DialogHeader>
          {options.data ? <TargetControls options={options.data} value={target} onChange={setTarget} noneLabel="Desktop default" disabled={busy || !allowed} /> : null}
          {options.isError ? <Notice>{errorText(options.error, "Could not load the cloud options. The desktop chooses the target.")}</Notice> : null}
          {!allowed ? <Notice>{offNote(canControl)}</Notice> : null}
          {move.isError ? <Notice variant="error">{errorText(move.error, "Could not move the session.")}</Notice> : null}
          <DialogFooter>
            <Button type="button" variant="ghost" disabled={busy} onClick={closeMove}>
              Cancel
            </Button>
            <Button type="button" disabled={busy || !allowed} aria-busy={busy} onClick={submit}>
              Move and start billing
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ContainClicks>
  );
}

function MoveToLocalDialog({ desktopId, session }: MoveDialogProps) {
  const { allowed, canControl } = useMoveAllowed();
  const move = useLinkMutation("cloud.moveToLocal");
  const refresh = useRefreshSessions(desktopId);
  return (
    <CloudConfirm
      open
      onOpenChange={onOpenChange}
      title="Continue this session locally?"
      description={COST_NOTICES.moveToLocal}
      confirmLabel="Continue locally"
      busy={move.isPending}
      confirmDisabled={!allowed}
      notice={allowed ? null : offNote(canControl)}
      error={move.isError ? errorText(move.error, "Could not move the session.") : null}
      onConfirm={() =>
        move.mutate(
          { sessionId: session.id },
          {
            onSuccess: () => {
              refresh();
              closeMove();
            },
          },
        )
      }
    />
  );
}

function CancelMoveDialog({ desktopId, session }: MoveDialogProps) {
  const { allowed, canControl } = useMoveAllowed();
  const cancel = useLinkMutation("cloud.cancelMove");
  const refresh = useRefreshSessions(desktopId);
  return (
    <CloudConfirm
      open
      onOpenChange={onOpenChange}
      title="Cancel this move?"
      description={CANCEL_MOVE_CONFIRM}
      confirmLabel="Cancel move"
      busy={cancel.isPending}
      confirmDisabled={!allowed}
      notice={allowed ? null : offNote(canControl)}
      error={cancel.isError ? errorText(cancel.error, "Could not cancel the move.") : null}
      onConfirm={() =>
        cancel.mutate(
          { sessionId: session.id },
          {
            onSuccess: () => {
              refresh();
              closeMove();
            },
          },
        )
      }
    />
  );
}

/**
 * Mounted beside a card badge or the header chip. It shows the request only when the request was opened on
 * this very host (see moveStore), and the request goes away with the host.
 */
export function MoveDialogHost({ desktopId, session }: MoveDialogProps) {
  const hostId = useId();
  const key = moveHostKey(desktopId, session.id);
  useEffect(() => registerHost(hostId, key), [hostId, key]);
  const { request, owner } = useMoveSnapshot();
  if (!request || owner !== hostId || moveHostKey(request.desktopId, request.session.id) !== key) return null;
  const props = { desktopId: request.desktopId, session: request.session };
  if (request.kind === "toCloud") return <MoveToCloudDialog {...props} />;
  if (request.kind === "toLocal") return <MoveToLocalDialog {...props} />;
  return <CancelMoveDialog {...props} />;
}
