// F-25, F-40: the menu in the session header: Changes, Rename, Copy Session ID, Settings, Archive and Delete,
// plus the items WS-C adds through the `session-menu-items` slot. Delete is hidden when `session-capabilities`
// says the session cannot be deleted (a cloud session); a read-only device does not see Rename, Archive or
// Delete. The Changes entry has no count: the session data carries no changed-file count. It reuses the
// dialogs of the lists.
import { Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { sessionCan } from "@/ui/hooks";
import { useCollapsedToolCalls } from "@/features/transcript/toolCallsSetting";
import { Icon } from "@/ui/Icon";
import { toast } from "@/ui/sonner";
import { ArchiveSessionDialog, RenameSessionDialog, useRefreshSessions } from "./SessionActionDialogs";
import { SessionMenuItemsSlot } from "./slots";

export function SessionHeaderMenu({ desktopId, session, readOnly }: { desktopId: string; session: SessionCard; readOnly: boolean }) {
  const navigate = useNavigate();
  const capabilities = useLinkState((state) => state.capabilities);
  const canDelete = sessionCan("delete", { target: session.target ?? null, desktopCapabilities: capabilities });
  const [renaming, setRenaming] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [collapsedCalls, setCollapsedCalls] = useCollapsedToolCalls();
  const remove = useLinkMutation("session.delete");
  const refresh = useRefreshSessions(desktopId);
  const canChanges = sessionCan("changes", { target: session.target ?? null, desktopCapabilities: capabilities });
  // The click of the menu item is the user gesture the clipboard needs.
  const copyId = () => {
    const write = globalThis.navigator?.clipboard?.writeText(session.id);
    if (!write) {
      toast.error("Could not copy the session ID");
      return;
    }
    write.then(
      () => toast.success("Session ID copied"),
      () => toast.error("Could not copy the session ID"),
    );
  };

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
          {canChanges ? (
            <DropdownMenuItem asChild>
              <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ desktopId, projectId: session.projectId, _splat: "" }}>
                <Icon name="list" size={18} />
                Changes
              </Link>
            </DropdownMenuItem>
          ) : null}
          {readOnly ? null : (
            <DropdownMenuItem onSelect={() => setRenaming(true)}>
              <Icon name="edit" size={18} />
              Rename
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onSelect={copyId}>
            <Icon name="copy" size={18} />
            Copy Session ID
          </DropdownMenuItem>
          <DropdownMenuItem asChild>
            <Link to="/d/$desktopId/settings" params={{ desktopId }}>
              <Icon name="settings" size={18} />
              Settings
            </Link>
          </DropdownMenuItem>
          {readOnly ? null : (
            <DropdownMenuItem onSelect={() => setArchiving(true)}>
              <Icon name="archive" size={18} />
              Archive
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            role="menuitemcheckbox"
            aria-checked={collapsedCalls}
            onSelect={() => setCollapsedCalls(!collapsedCalls)}
          >
            <Icon name="check" size={18} className={collapsedCalls ? undefined : "opacity-0"} />
            Collapse tool calls
          </DropdownMenuItem>
          {canDelete && !readOnly ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem destructive onSelect={() => setConfirming(true)}>
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
