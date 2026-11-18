// F-40 and U-16: the `archive-dialog` slot. It replaces WS-T's plain archive confirmation for EVERY
// session, so a local session keeps exactly that confirmation (same copy and request). A cloud session
// asks whether its instance keeps running or stops, with the cost notice (converting may wake the
// instance), and sends `session.archive {conversationId, cloud}`. Nothing here deletes anything.
import { useState } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { useLinkMutation } from "@/data";
import { useRefreshSessions } from "@/features/sessions/SessionActionDialogs";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Notice } from "@/ui/Notice";
import { Radio } from "@/ui/Radio";
import { ContainClicks } from "../CloudConfirm";
import { COST_NOTICES, READ_ONLY_NOTE } from "../copy";
import { isBillableKind, useCanControl } from "../model";
import { safeMessage, safeName } from "../safe";

interface ArchiveDialogProps {
  desktopId: string;
  session: SessionCard;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function CloudArchiveDialog(props: ArchiveDialogProps) {
  // Only an instance that bills (codespace, sandbox) gets the keep-or-stop choice. A remote machine (monocode-host)
  // is not a cloud instance: it gets the plain confirmation and no `cloud` field.
  return isBillableKind(props.session.target?.kind) ? <CloudChoiceDialog {...props} /> : <LocalArchiveDialog {...props} />;
}

function LocalArchiveDialog({ desktopId, session, open, onOpenChange }: ArchiveDialogProps) {
  const canControl = useCanControl();
  const archive = useLinkMutation("session.archive");
  const refresh = useRefreshSessions(desktopId);
  return (
    <ContainClicks>
      <ConfirmDialog
        open={open}
        onOpenChange={onOpenChange}
        title="Archive this session?"
        description={`"${safeName(session.title)}" moves out of the list. It stays on your desktop.`}
        confirmLabel="Archive"
        busy={archive.isPending}
        error={archive.isError ? "Could not archive the session." : null}
        notice={canControl ? null : READ_ONLY_NOTE}
        onConfirm={() => {
          if (!canControl) return;
          archive.mutate(
            { conversationId: session.id },
            {
              onSuccess: () => {
                refresh();
                onOpenChange(false);
              },
            },
          );
        }}
      />
    </ContainClicks>
  );
}

function CloudChoiceDialog({ desktopId, session, open, onOpenChange }: ArchiveDialogProps) {
  const canControl = useCanControl();
  const archive = useLinkMutation("session.archive");
  const refresh = useRefreshSessions(desktopId);
  const [choice, setChoice] = useState<"keep" | "stop">("stop");
  const run = () => {
    if (!canControl) return;
    archive.mutate(
      { conversationId: session.id, cloud: choice },
      {
        onSuccess: () => {
          refresh();
          onOpenChange(false);
        },
      },
    );
  };
  const options = [
    { value: "keep" as const, label: "Keep instance running", hint: "The instance keeps running and keeps billing." },
    { value: "stop" as const, label: "Stop instance", hint: "The instance stops. The desktop refuses this while another session is running on it." },
  ];
  return (
    <ContainClicks>
      <Dialog open={open} onOpenChange={(next) => (archive.isPending ? undefined : onOpenChange(next))}>
        <DialogContent showClose={false}>
          <DialogHeader className="pr-0">
            <DialogTitle>Archive this session?</DialogTitle>
            <DialogDescription>{`"${safeName(session.title)}" moves out of the list. Choose what happens to its cloud instance.`}</DialogDescription>
          </DialogHeader>
          <fieldset className="grid gap-1" disabled={!canControl || archive.isPending}>
            <legend className="sr-only">Instance after archiving</legend>
            {options.map((option) => (
              <label key={option.value} className="flex min-h-11 cursor-pointer items-start gap-3 rounded-row px-2 py-2 hover:bg-content-a04">
                <Radio name="archive-cloud-choice" value={option.value} checked={choice === option.value} onChange={() => setChoice(option.value)} className="mt-0.5" />
                <span className="flex flex-col">
                  <span className="text-small font-semibold">{option.label}</span>
                  <span className="text-meta text-muted-text">{option.hint}</span>
                </span>
              </label>
            ))}
          </fieldset>
          <Notice>{COST_NOTICES.archive}</Notice>
          {!canControl ? <Notice>{READ_ONLY_NOTE}</Notice> : null}
          {archive.isError ? <Notice variant="error">{safeMessage(archive.error.message, "Could not archive the session.")}</Notice> : null}
          <DialogFooter>
            <Button type="button" variant="ghost" disabled={archive.isPending} onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="button" disabled={!canControl || archive.isPending} aria-busy={archive.isPending} onClick={run}>
              Archive
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ContainClicks>
  );
}
