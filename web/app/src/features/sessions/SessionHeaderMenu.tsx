// F-25, F-40: the menu in the session header: Rename, Archive and Delete, plus the items WS-C adds through the
// `session-menu-items` slot. Delete is hidden when `session-capabilities` says the session cannot be deleted (a
// cloud session); every item is disabled for a read-only device. It reuses the dialogs of the lists.
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { sessionCan } from "@/ui/hooks";
import { Icon } from "@/ui/Icon";
import { ArchiveSessionDialog, RenameSessionDialog, useRefreshSessions } from "./SessionActionDialogs";
import { SessionMenuItemsSlot } from "./slots";

export function SessionHeaderMenu({ desktopId, session, readOnly }: { desktopId: string; session: SessionCard; readOnly: boolean }) {
  const navigate = useNavigate();
  const capabilities = useLinkState((state) => state.capabilities);
  const canDelete = sessionCan("delete", { target: session.target ?? null, desktopCapabilities: capabilities });
  const [renaming, setRenaming] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const remove = useLinkMutation("session.delete");
  const refresh = useRefreshSessions(desktopId);
  const reason = readOnly ? "This device is read-only" : undefined;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="Session actions"
            className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection pointer-fine:size-9"
          >
            <Icon name="more" size={18} />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <SessionMenuItemsSlot desktopId={desktopId} session={session} />
          <DropdownMenuItem disabled={readOnly} title={reason} onSelect={() => setRenaming(true)}>
            <Icon name="edit" size={18} />
            Rename
          </DropdownMenuItem>
          <DropdownMenuItem disabled={readOnly} title={reason} onSelect={() => setArchiving(true)}>
            <Icon name="archive" size={18} />
            Archive
          </DropdownMenuItem>
          {canDelete ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem destructive disabled={readOnly} title={reason} onSelect={() => setConfirming(true)}>
                <Icon name="trash" size={18} />
                Delete
              </DropdownMenuItem>
            </>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
      <RenameSessionDialog desktopId={desktopId} session={session} open={renaming} onOpenChange={setRenaming} />
      {archiving ? <ArchiveSessionDialog desktopId={desktopId} session={session} open onOpenChange={setArchiving} /> : null}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title="Delete this session?"
        description={`"${session.title}" is deleted on your desktop. This cannot be undone.`}
        confirmLabel="Delete"
        destructive
        busy={remove.isPending}
        error={remove.isError ? "Could not delete the session." : null}
        onConfirm={() =>
          remove.mutate(
            { conversationId: session.id },
            {
              onSuccess: () => {
                setConfirming(false);
                refresh();
                void navigate({ to: "/d/$desktopId/p/$projectId", params: { desktopId, projectId: session.projectId } });
              },
            },
          )
        }
      />
    </>
  );
}
