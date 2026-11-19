// F-24: a session as the desktop's Mono details panel lays out its card (ported from
// mobile/src/features/sessions/SessionCard.tsx, `detailed`): the title with the time or "Working..." on
// its right, then the branch with the model and its provider's mark on its right. The card links to the
// session; its menu offers the actions the `session-capabilities` predicate allows for its target.
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { SessionCard as SessionCardData } from "@monoptah/protocol";
import { useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { cn } from "@/lib/utils";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { sessionCan } from "@/ui/hooks";
import { TerminalSpinner } from "@/features/transcript/TerminalSpinner";
import { relativeTime } from "./relativeTime";
import { ArchiveSessionDialog, RenameSessionDialog, useRefreshSessions } from "./SessionActionDialogs";
import { SessionCardBadgeSlot, SessionMenuItemsSlot } from "./slots";

/** Every card is the same height, which is what lets the list be virtualized (the card is 68 plus 4 padding each side). */
export const SESSION_ROW_HEIGHT = 76;

export function statusLabel(session: Pick<SessionCardData, "waiting">): string {
  if (session.waiting === "approval") return "Needs your approval";
  if (session.waiting === "question") return "Has a question for you";
  return "Working";
}

export interface SessionCardProps {
  desktopId: string;
  session: SessionCardData;
  active?: boolean;
  /** `read` devices cannot delete. */
  readOnly?: boolean;
  now?: number;
}

export function SessionCard({ desktopId, session, active = false, readOnly = false, now }: SessionCardProps) {
  const model = session.modelName || session.model;
  const working = !!session.busy;
  const waiting = !!session.waiting;
  return (
    <div className="relative py-1" data-testid="session-card" data-session-id={session.id}>
      <Link
        to="/d/$desktopId/s/$sessionId"
        params={{ desktopId, sessionId: session.id }}
        aria-current={active ? "page" : undefined}
        aria-label={working || waiting ? `${session.title}, ${statusLabel(session).toLowerCase()}` : session.title}
        className={cn(
          "flex flex-col justify-center gap-1.5 rounded-row px-3.5 pr-14 text-content hover:bg-content-a04 active:bg-content-a04",
          active && "bg-selection hover:bg-selection",
        )}
        style={{ height: SESSION_ROW_HEIGHT - 8 }}
      >
        <span className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-body font-semibold">{session.title}</span>
          {waiting ? (
            <span className="shrink-0 text-meta text-caution">{session.waiting === "approval" ? "Needs approval" : "Question"}</span>
          ) : working ? (
            <span className="flex shrink-0 items-center gap-1 text-meta text-brand-text">
              <TerminalSpinner />
              Working...
            </span>
          ) : (
            <span className="shrink-0 text-meta tabular text-muted-text">{relativeTime(session.updatedAt, now)}</span>
          )}
        </span>
        <span className="flex items-center gap-2 text-meta text-muted-text">
          <span className="flex min-w-0 flex-1 items-center gap-1.5">
            <Icon name="branch" size={13} className="shrink-0 text-faint" />
            <span className={cn("truncate", !session.branch && "text-muted-text")}>{session.branch || "No branch"}</span>
            <SessionCardBadgeSlot desktopId={desktopId} session={session} />
          </span>
          <span className="flex max-w-[50%] shrink items-center gap-1.5">
            {model ? <span className="truncate">{model}</span> : null}
            <HarnessIcon harness={session.harness} className="size-3.5 shrink-0" />
          </span>
        </span>
      </Link>
      <SessionCardMenu desktopId={desktopId} session={session} readOnly={readOnly} />
    </div>
  );
}

function SessionCardMenu({ desktopId, session, readOnly }: { desktopId: string; session: SessionCardData; readOnly: boolean }) {
  const capabilities = useLinkState((state) => state.capabilities);
  const context = { target: session.target ?? null, desktopCapabilities: capabilities };
  const canFiles = sessionCan("files", context);
  const canChanges = sessionCan("changes", context);
  const canGit = sessionCan("git", context);
  const canDelete = sessionCan("delete", context);
  const [confirming, setConfirming] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const remove = useLinkMutation("session.delete");
  const refresh = useRefreshSessions(desktopId);
  const afterDelete = () => {
    setConfirming(false);
    refresh();
  };
  const params = { desktopId, projectId: session.projectId };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={`Actions for ${session.title}`}
            className="absolute top-1/2 right-1.5 inline-flex size-11 -translate-y-1/2 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
          >
            <Icon name="more" size={18} />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem asChild>
            <Link to="/d/$desktopId/s/$sessionId" params={{ desktopId, sessionId: session.id }}>
              <Icon name="chat" size={18} />
              Open
            </Link>
          </DropdownMenuItem>
          {canFiles ? (
            <DropdownMenuItem asChild>
              <Link to="/d/$desktopId/p/$projectId/files/$" params={{ ...params, _splat: "" }}>
                <Icon name="folder" size={18} />
                Files
              </Link>
            </DropdownMenuItem>
          ) : null}
          {canChanges ? (
            <DropdownMenuItem asChild>
              <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ ...params, _splat: "" }}>
                <Icon name="list" size={18} />
                Changes
              </Link>
            </DropdownMenuItem>
          ) : null}
          {canGit ? (
            <DropdownMenuItem asChild>
              <Link to="/d/$desktopId/p/$projectId/git" params={params}>
                <Icon name="branch" size={18} />
                Git
              </Link>
            </DropdownMenuItem>
          ) : null}
          <SessionMenuItemsSlot desktopId={desktopId} session={session} />
          {/* A read-only device does not see the actions that change the session (F-25). */}
          {readOnly ? null : (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setRenaming(true)}>
                <Icon name="edit" size={18} />
                Rename
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setArchiving(true)}>
                <Icon name="archive" size={18} />
                Archive
              </DropdownMenuItem>
            </>
          )}
          {canDelete && !readOnly ? (
            <DropdownMenuItem destructive onSelect={() => setConfirming(true)}>
              <Icon name="trash" size={18} />
              Delete
            </DropdownMenuItem>
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
        onConfirm={() => remove.mutate({ conversationId: session.id }, { onSuccess: afterDelete })}
      />
    </>
  );
}
