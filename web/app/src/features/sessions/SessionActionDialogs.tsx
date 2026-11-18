// F-25 and F-40: the dialogs behind a session's Rename and Archive menu items (the card menu of the lists
// and, later, the session header menu). Rename is a small form; Archive is the `archive-dialog` slot host,
// whose default is a plain confirmation (a cloud session also asks whether to keep or stop its instance).
import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { catalogKey, useLinkMutation } from "@/data";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Field } from "@/ui/Field";
import { ArchiveDialogSlot } from "./slots";

/** The title limit of `session.rename`. */
export const TITLE_MAX = 200;

/** Refreshes what a rename, archive or delete changes: the catalog and every paged list. */
export function useRefreshSessions(desktopId: string): () => void {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) });
    void queryClient.invalidateQueries({ queryKey: ["link", desktopId, "sessions.list"] });
  };
}

interface DialogProps {
  desktopId: string;
  session: SessionCard;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RenameSessionDialog({ desktopId, session, open, onOpenChange }: DialogProps) {
  // Re-mounted per opening, so the field starts from the current title.
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {open ? <RenameForm desktopId={desktopId} session={session} onOpenChange={onOpenChange} /> : null}
    </Dialog>
  );
}

function RenameForm({ desktopId, session, onOpenChange }: Omit<DialogProps, "open">) {
  const [title, setTitle] = useState(session.title);
  const rename = useLinkMutation("session.rename");
  const refresh = useRefreshSessions(desktopId);
  const trimmed = title.trim();
  const valid = trimmed.length > 0 && trimmed.length <= TITLE_MAX;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!valid || rename.isPending) return;
    if (trimmed === session.title) return onOpenChange(false);
    rename.mutate(
      { conversationId: session.id, title: trimmed },
      {
        onSuccess: () => {
          refresh();
          onOpenChange(false);
        },
      },
    );
  };
  return (
    <DialogContent showClose={false}>
      <form onSubmit={submit} className="grid gap-4">
        <DialogHeader className="pr-0">
          <DialogTitle>Rename session</DialogTitle>
          <DialogDescription className="sr-only">Give the session a new title.</DialogDescription>
        </DialogHeader>
        <Field
          label="Title"
          value={title}
          maxLength={TITLE_MAX}
          autoComplete="off"
          autoFocus
          onChange={(event) => setTitle(event.target.value)}
          error={rename.isError ? "Could not rename the session." : null}
        />
        <DialogFooter>
          <Button type="button" variant="ghost" disabled={rename.isPending} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" disabled={!valid || rename.isPending} aria-busy={rename.isPending}>
            Save
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  );
}

/** True for a session that runs on a cloud instance, which archiving must decide to keep or stop. */
export const isCloudSession = (session: Pick<SessionCard, "target">): boolean => !!session.target && session.target.kind !== "local";

function DefaultArchiveDialog({ desktopId, session, open, onOpenChange }: DialogProps) {
  const archive = useLinkMutation("session.archive");
  const refresh = useRefreshSessions(desktopId);
  const cloud = isCloudSession(session);
  const run = (choice?: "keep" | "stop") =>
    archive.mutate(
      { conversationId: session.id, ...(choice ? { cloud: choice } : {}) },
      {
        onSuccess: () => {
          refresh();
          onOpenChange(false);
        },
      },
    );
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Archive this session?"
      description={
        cloud
          ? `"${session.title}" moves out of the list. Choose whether its cloud instance keeps running or stops.`
          : `"${session.title}" moves out of the list. It stays on your desktop.`
      }
      confirmLabel={cloud ? "Archive and stop instance" : "Archive"}
      secondary={cloud ? { label: "Archive, keep running", onClick: () => run("keep") } : undefined}
      busy={archive.isPending}
      error={archive.isError ? "Could not archive the session." : null}
      onConfirm={() => run(cloud ? "stop" : undefined)}
    />
  );
}

/** The archive confirmation: what the `archive-dialog` slot registers, else the default dialog. */
export function ArchiveSessionDialog(props: DialogProps) {
  return <ArchiveDialogSlot {...props} fallback={<DefaultArchiveDialog {...props} />} />;
}
