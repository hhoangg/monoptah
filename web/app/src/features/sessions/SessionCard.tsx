// F-24: a session as the desktop's Mono details panel lays out its card (ported from
// mobile/src/features/sessions/SessionCard.tsx, `detailed`): the title with the time or "Working..." on
// its right, then the branch with the model and its provider's mark on its right. The card links to the
// session; its menu offers the actions the `session-capabilities` predicate allows for its target.
//
// The card adapts to its own width with a container query (the session list sidebar is 240-560px, the phone page is
// the full width): under 18rem (a list under 320px) the model name gives way to the harness mark and "Working..." loses its dots,
// so the title, the branch and the target chip keep their room and truncate with an ellipsis. From 18rem, which
// includes the default 320px list, the phone page and tablet, the card shows the model and the status.
import { Link } from "@tanstack/react-router";
import { useRef, useState } from "react";
import type { SessionCard as SessionCardData } from "@monoptah/protocol";
import { useCloseSessionTab } from "@/features/layout/tabs/useCloseSessionTab";
import { useCatalog, useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { LONG_PRESS_SURFACE, useLongPress } from "@/lib/useLongPress";
import { cn } from "@/lib/utils";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { COARSE_POINTER_QUERY, useMediaQuery } from "@/ui/useMediaQuery";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { sessionCan } from "@/ui/hooks";
import { TerminalSpinner } from "@/features/transcript/TerminalSpinner";
import { canEditFolders, folderOf, projectFolders } from "./folders";
import { relativeTime } from "./relativeTime";
import { ArchiveSessionDialog, RenameSessionDialog, useRefreshSessions } from "./SessionActionDialogs";
import { AddToFolderDialog, FolderErrorDialog } from "./SessionFolderDialogs";
import { SessionCardBadgeSlot, SessionMenuItemsSlot } from "./slots";
import { useSwipeReveal } from "./useSwipeReveal";

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

/** Width of the two buttons a swipe reveals (2 x 72 px) plus the gaps (3 x 8 px). */
const SWIPE_ACTIONS_WIDTH = 168;

export function SessionCard({ desktopId, session, active = false, readOnly = false, now }: SessionCardProps) {
  const model = session.modelName || session.model;
  const working = !!session.busy;
  const waiting = !!session.waiting;
  // Touch layouts (a coarse primary pointer: phone, tablet): no menu button. A long press opens the same menu, and a
  // swipe from right to left reveals Archive and Delete. A mouse layout keeps the button and has neither.
  const touch = useMediaQuery(COARSE_POINTER_QUERY);
  const actions = useSessionRowActions(session, readOnly);
  const [menuOpen, setMenuOpen] = useState(false);
  const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null);
  const rowRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const openMenuAt = (x: number, y: number) => {
    const box = surfaceRef.current?.getBoundingClientRect();
    setAnchor({ x: x - (box?.left ?? 0), y: y - (box?.top ?? 0) });
    setMenuOpen(true);
  };
  const swipeable = touch && (actions.canArchive || actions.canDelete);
  const swipe = useSwipeReveal({ id: session.id, enabled: swipeable, width: SWIPE_ACTIONS_WIDTH, rowRef });
  const press = useLongPress({ enabled: touch && !swipe.open && !menuOpen, onLongPress: ({ x, y }) => openMenuAt(x, y) });
  const handlers = touch
    ? {
        onPointerDown: (event: React.PointerEvent<HTMLElement>) => {
          swipe.bind.onPointerDown(event);
          press.bind.onPointerDown(event);
        },
        onPointerMove: (event: React.PointerEvent<HTMLElement>) => {
          swipe.bind.onPointerMove(event);
          press.bind.onPointerMove(event);
        },
        onPointerUp: (event: React.PointerEvent<HTMLElement>) => {
          swipe.bind.onPointerUp(event);
          press.bind.onPointerUp(event);
        },
        onPointerCancel: (event: React.PointerEvent<HTMLElement>) => {
          swipe.bind.onPointerCancel(event);
          press.bind.onPointerCancel(event);
        },
        onClickCapture: (event: React.MouseEvent<HTMLElement>) => {
          swipe.bind.onClickCapture(event);
          press.bind.onClickCapture(event);
        },
        // Android Chrome fires `contextmenu` on a long press (and a trackpad or mouse right-click on a tablet does):
        // the menu is ours, not the browser's.
        onContextMenu: (event: React.MouseEvent<HTMLElement>) => {
          event.preventDefault();
          if (!menuOpen) openMenuAt(event.clientX, event.clientY);
        },
      }
    : {};

  return (
    <div ref={rowRef} className={cn("@container relative py-1", touch && "overflow-x-clip")} data-testid="session-card" data-session-id={session.id} data-swipe-open={swipe.open ? "true" : undefined}>
      <div
        ref={surfaceRef}
        className={cn("relative", touch && cn("touch-pan-y", LONG_PRESS_SURFACE, !swipe.dragging && "transition-transform duration-200 ease-out motion-reduce:transition-none"))}
        style={touch ? { transform: `translate3d(${swipe.offset}px, 0, 0)` } : undefined}
        {...handlers}
      >
        <Link
          to="/d/$desktopId/s/$sessionId"
          params={{ desktopId, sessionId: session.id }}
          aria-current={active ? "page" : undefined}
          aria-label={working || waiting ? `${session.title}, ${statusLabel(session).toLowerCase()}` : session.title}
          draggable={touch ? false : undefined}
          tabIndex={swipe.open ? -1 : undefined}
          className={cn(
            "flex flex-col justify-center gap-1.5 rounded-row px-3.5 pr-14 text-content hover:bg-content-a04 active:bg-content-a04",
            touch && "pr-3.5",
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
                <span className="@max-[18rem]:hidden">Working...</span>
                <span className="hidden @max-[18rem]:inline">Working</span>
              </span>
            ) : (
              <span className="shrink-0 text-meta tabular text-muted-text">{relativeTime(session.updatedAt, now)}</span>
            )}
          </span>
          <span className="flex items-center gap-2 text-meta text-muted-text">
            <span className="flex min-w-0 flex-1 items-center gap-1.5">
              <Icon name="branch" size={13} className="shrink-0 text-faint" />
              <span className={cn("min-w-0 shrink truncate", !session.branch && "text-muted-text")}>{session.branch || "No branch"}</span>
              <span className="flex min-w-0 shrink">
                <SessionCardBadgeSlot desktopId={desktopId} session={session} />
              </span>
            </span>
            <span className="flex max-w-[50%] shrink items-center gap-1.5 @max-[18rem]:max-w-none" title={model || undefined}>
              {model ? <span className="truncate @max-[18rem]:hidden">{model}</span> : null}
              <HarnessIcon harness={session.harness} className="size-3.5 shrink-0" />
            </span>
          </span>
        </Link>
        <SessionCardMenu desktopId={desktopId} session={session} actions={actions} touch={touch} open={menuOpen} onOpenChange={(next) => {
            setMenuOpen(next);
            if (!next) setAnchor(null);
          }}
          anchor={anchor} />
        {swipeable ? (
          <div data-swipe-actions className={cn("absolute top-0 left-full flex h-full items-stretch gap-2 pl-2", swipe.offset === 0 && "invisible")} inert={!swipe.open}>
            {actions.canArchive ? (
              <SwipeButton
                label="Archive"
                icon="archive"
                aria-label={`Archive ${session.title}`}
                className="bg-surface-raised text-content"
                onClick={() => {
                  swipe.close();
                  actions.setArchiving(true);
                }}
              />
            ) : null}
            {actions.canDelete ? (
              <SwipeButton
                label="Delete"
                icon="trash"
                aria-label={`Delete ${session.title}`}
                className="bg-destructive text-inverse"
                onClick={() => {
                  swipe.close();
                  actions.setConfirming(true);
                }}
              />
            ) : null}
          </div>
        ) : null}
      </div>
      <SessionRowDialogs desktopId={desktopId} session={session} readOnly={readOnly} actions={actions} />
    </div>
  );
}

function SwipeButton({ label, icon, className, ...props }: { label: string; icon: "archive" | "trash"; className: string } & React.ComponentProps<"button">) {
  return (
    <button type="button" className={cn("flex w-[4.5rem] flex-col items-center justify-center gap-1 rounded-row text-meta font-medium active:opacity-60", className)} {...props}>
      <Icon name={icon} size={20} />
      {label}
    </button>
  );
}

/** What a session row can do, with the state of the dialogs behind Rename, Add to folder, Archive and Delete. Shared by the menu and the swipe buttons. */
function useSessionRowActions(session: SessionCardData, readOnly: boolean) {
  const capabilities = useLinkState((state) => state.capabilities);
  const context = { target: session.target ?? null, desktopCapabilities: capabilities };
  const [confirming, setConfirming] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [filing, setFiling] = useState(false);
  return {
    canFiles: sessionCan("files", context),
    canChanges: sessionCan("changes", context),
    canGit: sessionCan("git", context),
    // A read-only device does not see the actions that change the session (F-25).
    canArchive: !readOnly,
    canDelete: sessionCan("delete", context) && !readOnly,
    confirming,
    setConfirming,
    renaming,
    setRenaming,
    archiving,
    setArchiving,
    filing,
    setFiling,
  };
}

type SessionRowActions = ReturnType<typeof useSessionRowActions>;

interface SessionCardMenuProps {
  desktopId: string;
  session: SessionCardData;
  actions: SessionRowActions;
  /** Touch layout: no button, the menu is opened by a long press at `anchor` (px, relative to the row). */
  touch: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  anchor: { x: number; y: number } | null;
}

function SessionCardMenu({ desktopId, session, actions, touch, open, onOpenChange, anchor }: SessionCardMenuProps) {
  const { canFiles, canChanges, canGit, canArchive, canDelete, setConfirming, setRenaming, setArchiving, setFiling } = actions;
  // Kept here, not in the menu items, which unmount when the menu closes.
  const unfile = useLinkMutation("folder.move");
  const refresh = useRefreshSessions(desktopId);
  const params = { desktopId, projectId: session.projectId };

  return (
    <>
      <DropdownMenu open={open} onOpenChange={onOpenChange}>
        <DropdownMenuTrigger asChild>
          {touch ? (
            // No visible button on touch, but the menu stays reachable for VoiceOver and a keyboard: a visually hidden
            // button, which also anchors a long-press menu under the finger.
            <button
              type="button"
              aria-label={`Actions for ${session.title}`}
              data-testid="session-menu-anchor"
              className="sr-only"
              style={anchor ? { left: anchor.x, top: anchor.y } : { right: 6, top: "50%" }}
            />
          ) : (
            <button
              type="button"
              aria-label={`Actions for ${session.title}`}
              className="absolute top-1/2 right-1.5 inline-flex size-11 -translate-y-1/2 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
            >
              <Icon name="moreVertical" size={18} />
            </button>
          )}
        </DropdownMenuTrigger>
        <DropdownMenuContent align={touch && anchor ? "center" : "end"} sideOffset={touch && anchor ? 14 : undefined}>
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
          {canArchive ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setRenaming(true)}>
                <Icon name="edit" size={18} />
                Rename
              </DropdownMenuItem>
              <FolderMenuItems
                session={session}
                onAdd={() => setFiling(true)}
                onRemove={() => unfile.mutate({ projectId: session.projectId, conversationId: session.id, folderId: null }, { onSuccess: refresh })}
              />
              <DropdownMenuItem onSelect={() => setArchiving(true)}>
                <Icon name="archive" size={18} />
                Archive
              </DropdownMenuItem>
            </>
          ) : null}
          {canDelete ? (
            <DropdownMenuItem destructive onSelect={() => setConfirming(true)}>
              <Icon name="trash" size={18} />
              Delete
            </DropdownMenuItem>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
      <FolderErrorDialog open={unfile.isError} onClose={unfile.reset} />
    </>
  );
}

/** The dialogs behind the menu and the swipe buttons. Rendered outside the swiped surface so they do not move with it. */
function SessionRowDialogs({ desktopId, session, readOnly, actions }: { desktopId: string; session: SessionCardData; readOnly: boolean; actions: SessionRowActions }) {
  const remove = useLinkMutation("session.delete");
  const refresh = useRefreshSessions(desktopId);
  const closeTab = useCloseSessionTab(desktopId);
  const { confirming, setConfirming, renaming, setRenaming, archiving, setArchiving, filing, setFiling } = actions;
  const afterDelete = () => {
    setConfirming(false);
    refresh();
    closeTab(session);
  };
  return (
    <>
      <RenameSessionDialog desktopId={desktopId} session={session} open={renaming} onOpenChange={setRenaming} />
      {readOnly ? null : <AddToFolderDialog desktopId={desktopId} session={session} open={filing} onOpenChange={setFiling} />}
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

/** "Add to folder" and, for a session in one, "Remove from folder". Only rendered while the menu is open. */
function FolderMenuItems({ session, onAdd, onRemove }: { session: SessionCardData; onAdd: () => void; onRemove: () => void }) {
  const catalog = useCatalog();
  // An older desktop sends no folders, and a remote-machine project's are refused: no folder items for those.
  if (!canEditFolders(catalog.data?.folders, session.projectId)) return null;
  const current = folderOf(projectFolders(catalog.data?.folders, session.projectId), session.id);
  return (
    <>
      <DropdownMenuItem onSelect={onAdd}>
        <Icon name="folder" size={18} />
        {current ? "Move to folder" : "Add to folder"}
      </DropdownMenuItem>
      {current ? (
        <DropdownMenuItem onSelect={onRemove}>
          <Icon name="minus" size={18} />
          Remove from folder
        </DropdownMenuItem>
      ) : null}
    </>
  );
}
