// The dialogs behind the move menu items (U-19) and the host that mounts them. Each move needs a cost notice
// and an explicit confirm. Cancel move says the workspace is kept and never offers deleting anything. The
// progress of the 7 steps is the step list of `CloudComposerTop` (composer-top), not here.
import { useEffect, useId, useState } from "react";
import type { SessionCard, SessionStartTarget } from "@monoptah/protocol";
import { useLinkMutation, useLinkQuery } from "@/data";
import { useRefreshSessions } from "@/features/sessions/SessionActionDialogs";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Notice } from "@/ui/Notice";
import { CANCEL_MOVE_CONFIRM, COST_NOTICES, MOVE_DISABLED_NOTE } from "../copy";
import { TargetControls } from "../start/TargetControls";
import { closeMove, registerHost, useMoveSnapshot } from "./moveStore";

interface MoveDialogProps {
  desktopId: string;
  session: SessionCard;
}

/** The desktop refusal as plain text; `disabled` (the toggles are off) gets its own explanation. */
function errorText(error: { code?: string | undefined; message: string }, fallback: string): string {
  if (error.code === "disabled") return MOVE_DISABLED_NOTE;
  return error.message.trim() ? error.message : fallback;
}

function onOpenChange(open: boolean) {
  if (!open) closeMove();
}

function MoveToCloudDialog({ desktopId, session }: MoveDialogProps) {
  const options = useLinkQuery("cloud.startOptions", { projectId: session.projectId });
  const move = useLinkMutation("cloud.moveToCloud");
  const refresh = useRefreshSessions(desktopId);
  const [target, setTarget] = useState<SessionStartTarget | null>(null);
  const busy = move.isPending;
  const submit = () =>
    move.mutate(
      { sessionId: session.id, ...(target ? { target } : {}) },
      {
        onSuccess: () => {
          refresh();
          closeMove();
        },
      },
    );
  return (
    <Dialog open onOpenChange={(open) => (busy ? undefined : onOpenChange(open))}>
      <DialogContent showClose={false}>
        <DialogHeader className="pr-0">
          <DialogTitle>Move this session to the cloud?</DialogTitle>
          <DialogDescription>{COST_NOTICES.moveToCloud}</DialogDescription>
        </DialogHeader>
        {options.data ? <TargetControls options={options.data} value={target} onChange={setTarget} noneLabel="Desktop default" disabled={busy} /> : null}
        {options.isError ? <Notice>{errorText(options.error, "Could not load the cloud options. The desktop chooses the target.")}</Notice> : null}
        {move.isError ? <Notice variant="error">{errorText(move.error, "Could not move the session.")}</Notice> : null}
        <DialogFooter>
          <Button type="button" variant="ghost" disabled={busy} onClick={closeMove}>
            Cancel
          </Button>
          <Button type="button" disabled={busy} aria-busy={busy} onClick={submit}>
            Move and start billing
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function MoveToLocalDialog({ desktopId, session }: MoveDialogProps) {
  const move = useLinkMutation("cloud.moveToLocal");
  const refresh = useRefreshSessions(desktopId);
  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title="Continue this session locally?"
      description={COST_NOTICES.moveToLocal}
      confirmLabel="Continue locally"
      busy={move.isPending}
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
  const cancel = useLinkMutation("cloud.cancelMove");
  const refresh = useRefreshSessions(desktopId);
  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title="Cancel this move?"
      description={CANCEL_MOVE_CONFIRM}
      confirmLabel="Cancel move"
      busy={cancel.isPending}
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

/** Mounted beside a card badge or the header chip; renders the dialog of a request for its session when it is the owner. */
export function MoveDialogHost({ session }: MoveDialogProps) {
  const sessionId = session.id;
  const hostId = useId();
  useEffect(() => registerHost(hostId, sessionId), [hostId, sessionId]);
  const { request, hosts } = useMoveSnapshot();
  if (!request || request.session.id !== sessionId) return null;
  const owner = [...hosts].reverse().find((host) => host.sessionId === sessionId);
  if (owner?.id !== hostId) return null;
  const props = { desktopId: request.desktopId, session: request.session };
  if (request.kind === "toCloud") return <MoveToCloudDialog {...props} />;
  if (request.kind === "toLocal") return <MoveToLocalDialog {...props} />;
  return <CancelMoveDialog {...props} />;
}
