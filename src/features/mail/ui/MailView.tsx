import { useRef, useState } from "react";
import { Mail, MessageSquarePlus, Send } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { useDragResize } from "../../../shared/hooks/useDragResize";
import { IS_MAC } from "../../../platform/tauri/platform";
import { OverlayNav } from "../../../app/shell/TitleBar";
import { WindowControls } from "../../../app/shell/WindowControls";
import {
  LIST_WIDTH_DEFAULT,
  listWidth,
  selectedFolder,
  selectedMailbox,
  useReadingPaneSide,
  useSelectedFolder,
  useSelectedMailbox,
} from "../model/mailPrefs";
import type { ComposeRequest } from "../model/mailReply";
import type { FolderRow, MatchMode, Rule } from "../model/mailTypes";
import { useMailOutbox } from "../model/mailOutbox";
import { parseFolderId, useMailFolders } from "../model/useMailFolders";
import { useMailStatus } from "../model/useMailStatus";
import { ComposeSheet, preloadMailEditor } from "./ComposeSheet";
import { MailFolderList, groupMailboxesByDomain } from "./MailFolderList";
import {
  MailThreadList,
  useMailThreadList,
  type ListBase,
} from "./MailThreadList";
import { MailThreadRead } from "./MailThreadRead";
import { OutboxSheet } from "./OutboxSheet";
import { SmartFolderSheet } from "./SmartFolderSheet";

type Sheet =
  | { kind: "new"; rules: Rule[]; matchMode: MatchMode }
  | { kind: "edit"; folder: FolderRow };

type Props = {
  besideRail?: boolean;
  compactRail?: boolean;
  onClose: () => void;
  onToggleSidebar?: () => void;
};

const LIST_MIN_WIDTH = 280;
const LIST_MAX_WIDTH = 640;
// Keeps the reading pane usable however wide the window is dragged.
const READING_MIN_WIDTH = 320;
const FOLDER_COLUMN_WIDTH = 232;

/**
 * Four-pane mail shell: the app's rail (outside this view), the folder column,
 * the thread list and the reading pane. The panes are placeholders; the folder
 * list, thread list and reader fill them in.
 */
export function MailView({
  besideRail = false,
  compactRail = false,
  onClose,
  onToggleSidebar,
}: Props) {
  const side = useReadingPaneSide();
  const { status, error, refresh } = useMailStatus();
  const mailboxPref = useSelectedMailbox();
  const folderPref = useSelectedFolder();
  const {
    folders,
    error: foldersError,
    refresh: refreshFolders,
  } = useMailFolders();
  const [threadId, setThreadId] = useState<number | null>(null);
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const [compose, setCompose] = useState<{
    key: number;
    request: ComposeRequest;
  } | null>(null);
  const outbox = useMailOutbox();
  const [outboxOpen, setOutboxOpen] = useState(false);
  const unconfirmed =
    outbox.rows?.filter((r) => r.state === "unconfirmed").length ?? 0;
  const composeSeq = useRef(0);
  const openCompose = (request: ComposeRequest) =>
    setCompose({ key: ++composeSeq.current, request });
  const mailboxes = status
    ? groupMailboxesByDomain(status.sources).flatMap((g) =>
        g.mailboxes.map((m) => m.mailbox),
      )
    : [];
  // A saved id can outlive its mailbox; fall back to all mail rather than
  // querying a scope that no longer exists.
  const mailboxId =
    mailboxPref !== null && mailboxes.some((m) => String(m.id) === mailboxPref)
      ? Number(mailboxPref)
      : null;
  // A saved folder can be deleted elsewhere; like a mailbox, fall back to all
  // mail rather than querying an id that no longer exists.
  const prefFolderId = parseFolderId(folderPref);
  const folderId =
    prefFolderId !== null && folders?.some((f) => f.id === prefFolderId)
      ? prefFolderId
      : null;
  const folderPending =
    prefFolderId !== null && folders === null && foldersError === null;
  const mailboxAddress = mailboxes.find((m) => m.id === mailboxId)?.address;
  const scope: ListBase | null = folderPending
    ? null
    : folderId !== null
      ? { kind: "folder", folderId }
      : mailboxId !== null && mailboxAddress
        ? { kind: "mailbox", mailboxId, address: mailboxAddress }
        : { kind: "all" };
  const list = useMailThreadList(scope);
  const pick = (mailbox: string | null, folder: string | null) => {
    selectedMailbox.save(mailbox);
    selectedFolder.save(folder);
    setThreadId(null);
  };
  const resize = useDragResize({
    min: LIST_MIN_WIDTH,
    max: () =>
      Math.max(
        LIST_MIN_WIDTH,
        Math.min(
          LIST_MAX_WIDTH,
          window.innerWidth - FOLDER_COLUMN_WIDTH - READING_MIN_WIDTH,
        ),
      ),
    defaultWidth: LIST_WIDTH_DEFAULT,
    // Read once: the pane width is set imperatively, so a live subscription
    // would re-render without moving anything.
    initial: listWidth.load(),
    onCommit: listWidth.save,
  });
  // Width is a column only beside the reader; below it, or alone, the list
  // spans the row and a fixed width would leave a gap.
  const listBeside = side === "right";

  return (
    <div
      role="region"
      aria-label="Mail"
      data-app-mail
      className="flex min-h-0 min-w-0 flex-1 flex-col text-content"
    >
      <div
        className="flex h-10 shrink-0 select-none items-center border-b border-stroke"
        data-tauri-drag-region="deep"
      >
        {IS_MAC && compactRail ? <div className="w-4 shrink-0" /> : null}
        {IS_MAC && !besideRail ? <div className="w-[78px] shrink-0" /> : null}
        {besideRail ? null : (
          <OverlayNav onBack={onClose} onToggleSidebar={onToggleSidebar} />
        )}
        <div className="flex min-w-0 flex-1 items-center gap-2 px-3 text-[13px]">
          <Mail
            className="size-3.5 shrink-0 text-content/45"
            strokeWidth={1.75}
          />
          <span className="min-w-0 truncate text-content">Mail</span>
        </div>
        <div className="flex shrink-0 items-center gap-1.5 pr-2">
          <SecondaryButton
            danger={unconfirmed > 0}
            aria-label={
              unconfirmed > 0 ? `Outbox, ${unconfirmed} to check` : "Outbox"
            }
            onClick={() => setOutboxOpen(true)}
          >
            <Send className="size-3" />
            Outbox
            {outbox.rows && outbox.rows.length > 0 ? (
              <span className="tabular-nums text-content/50">
                {outbox.rows.length}
              </span>
            ) : null}
          </SecondaryButton>
          <SecondaryButton
            onPointerEnter={preloadMailEditor}
            onFocus={preloadMailEditor}
            onClick={() => openCompose({ kind: "new" })}
          >
            <MessageSquarePlus className="size-3" />
            Compose
          </SecondaryButton>
        </div>
        {IS_MAC ? null : <WindowControls />}
      </div>
      <div className="flex min-h-0 min-w-0 flex-1">
        <aside
          aria-label="Folders"
          data-mail-pane="folders"
          style={{ width: FOLDER_COLUMN_WIDTH }}
          className="flex shrink-0 flex-col border-r border-stroke"
        >
          <MailFolderList
            status={status}
            error={error}
            mailboxId={mailboxId}
            folderId={folderId}
            smartFolders={
              folders?.map((f) => ({
                id: f.id,
                name: f.name,
                color: f.color,
                unread: f.countUnread ? f.unreadCount : 0,
              })) ?? null
            }
            foldersError={foldersError}
            onNewFolder={() =>
              setSheet({ kind: "new", rules: [], matchMode: "all" })
            }
            onSelectAll={() => pick(null, null)}
            onSelectMailbox={(id) => pick(String(id), null)}
            onSelectFolder={(id) => pick(null, String(id))}
            onSynced={() => void refresh()}
          />
        </aside>
        <div
          className={`flex min-h-0 min-w-0 flex-1 ${
            side === "bottom" ? "flex-col" : ""
          }`}
        >
          <section
            // Remount on layout change: the drag hook writes an inline width
            // that nothing clears when the list stops being a column.
            key={listBeside ? "beside" : "stacked"}
            aria-label="Messages"
            data-mail-pane="threads"
            ref={listBeside ? resize.setPaneRef : undefined}
            className={`relative flex min-h-0 flex-col ${
              listBeside
                ? "shrink-0 border-r border-stroke"
                : side === "bottom"
                  ? "h-2/5 shrink-0 border-b border-stroke"
                  : "min-w-0 flex-1"
            }`}
          >
            {listBeside ? (
              <div
                role="separator"
                aria-orientation="vertical"
                aria-label="Resize message list"
                className={`absolute inset-y-0 -right-px z-10 w-1.5 cursor-col-resize touch-none ${
                  resize.dragging ? "bg-content/15" : "hover:bg-content/10"
                }`}
                onPointerDown={resize.onPointerDown}
                onDoubleClick={resize.onDoubleClick}
              />
            ) : null}
            <MailThreadList
              list={list}
              scope={scope ?? { kind: "all" }}
              mailboxes={mailboxes}
              selectedId={threadId}
              onSelect={(thread) => setThreadId(thread.id)}
              onSaveAsFolder={(rules, matchMode) =>
                setSheet({ kind: "new", rules, matchMode })
              }
              onEditFolder={() => {
                const folder = folders?.find((f) => f.id === folderId);
                if (folder) setSheet({ kind: "edit", folder });
              }}
            />
          </section>
          {side === "off" ? null : (
            <section
              aria-label="Reading pane"
              data-mail-pane="reader"
              className="flex min-h-0 min-w-0 flex-1 flex-col"
            >
              <MailThreadRead threadId={threadId} onCompose={openCompose} />
            </section>
          )}
        </div>
      </div>
      {compose ? (
        <ComposeSheet
          // Remount per opening: its fields start from the request.
          key={compose.key}
          request={compose.request}
          onClose={() => {
            setCompose(null);
            // A save emits nothing, so this is when the list learns of it.
            void outbox.reload();
          }}
        />
      ) : null}
      {outboxOpen ? (
        <OutboxSheet
          rows={outbox.rows}
          error={outbox.error}
          onChanged={() => void outbox.reload()}
          onClose={() => setOutboxOpen(false)}
          onOpen={(row) => {
            setOutboxOpen(false);
            openCompose({ kind: "draft", row });
          }}
        />
      ) : null}
      {sheet ? (
        <SmartFolderSheet
          // Remount per opening: its fields start from the folder or filters.
          key={sheet.kind === "edit" ? `edit-${sheet.folder.id}` : "new"}
          folder={sheet.kind === "edit" ? sheet.folder : null}
          initial={
            sheet.kind === "new"
              ? { rules: sheet.rules, matchMode: sheet.matchMode }
              : null
          }
          suggestions={{
            mailboxes: mailboxes.map((m) => m.address),
            domains: [...new Set(mailboxes.map((m) => m.domain))],
            sources: status?.sources.map((s) => s.label) ?? [],
          }}
          onClose={() => setSheet(null)}
          onSaved={(saved) => {
            setSheet(null);
            void refreshFolders();
            void refresh();
            list.reload();
            if (sheet.kind === "new") pick(null, String(saved.id));
          }}
          onDeleted={(id) => {
            setSheet(null);
            void refreshFolders();
            void refresh();
            if (id === folderId) pick(null, null);
          }}
        />
      ) : null}
    </div>
  );
}
