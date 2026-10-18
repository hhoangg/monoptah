import { useEffect, useRef, useState } from "react";
import {
  ChevronLeft,
  Mail,
  MessageSquarePlus,
  Send,
} from "../../../shared/ui/icons";
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
import { mailThread } from "../model/mail";
import { mailKeyAction, type MailKeyAction } from "../model/mailKeys";
import type { ComposeRequest } from "../model/mailReply";
import type { FolderRow, MatchMode, Rule, ThreadRow } from "../model/mailTypes";
import { useMailOutbox } from "../model/mailOutbox";
import {
  ARCHIVE_PREF,
  parseFolderId,
  useMailFolders,
} from "../model/useMailFolders";
import { useMailStatus } from "../model/useMailStatus";
import { ComposeSheet, preloadMailEditor } from "./ComposeSheet";
import {
  DeleteMailDialog,
  mailDelete,
  type DeleteReport,
} from "./DeleteMailDialog";
import { MailFolderList, groupMailboxesByDomain } from "./MailFolderList";
import {
  MailThreadList,
  useMailThreadList,
  type ListBase,
} from "./MailThreadList";
import { MailSourceErrors, sourceSetup } from "./MailNotices";
import { MailThreadRead, sortedMessages } from "./MailThreadRead";
import { OutboxSheet } from "./OutboxSheet";
import { SmartFolderSheet } from "./SmartFolderSheet";

type Sheet =
  | { kind: "new"; rules: Rule[]; matchMode: MatchMode }
  | { kind: "edit"; folder: FolderRow };

type Props = {
  /** Drops the view's own header: the new layout's title bar carries the chrome. */
  tabbed?: boolean;
  besideRail?: boolean;
  compactRail?: boolean;
  onClose: () => void;
  onToggleSidebar?: () => void;
};

/** Both facts the button shows, so a screen reader hears what a sighted user sees. */
const outboxLabel = (total: number, unconfirmed: number) =>
  total === 0
    ? "Outbox"
    : `Outbox, ${total} ${total === 1 ? "message" : "messages"}${
        unconfirmed > 0 ? `, ${unconfirmed} to check` : ""
      }`;

const OPEN_SURFACE = '[role="dialog"], [role="menu"], [role="listbox"]';

/** What an archive left behind, in the dialog's words; null when it was clean. */
function archiveNotice(report: DeleteReport): string | null {
  const parts: string[] = [];
  if (report.filesLeft > 0) {
    parts.push(
      `Archived, but downloaded attachment files for ${report.filesLeft} ${
        report.filesLeft === 1 ? "message" : "messages"
      } could not be removed from this computer.`,
    );
  }
  parts.push(...report.errors);
  return parts.length > 0 ? parts.join(" ") : null;
}

const errorText = (err: unknown) =>
  err instanceof Error ? err.message : String(err);

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
  tabbed = false,
  besideRail = false,
  compactRail = false,
  onClose,
  onToggleSidebar,
}: Props) {
  const side = useReadingPaneSide();
  const { status, error, refresh, progress } = useMailStatus();
  const mailboxPref = useSelectedMailbox();
  const folderPref = useSelectedFolder();
  const {
    folders,
    error: foldersError,
    refresh: refreshFolders,
  } = useMailFolders();
  const [threadId, setThreadId] = useState<number | null>(null);
  // The keyboard's row: j/k move it, Enter opens it. Clicking sets both.
  const [cursorId, setCursorId] = useState<number | null>(null);
  const [deleteRow, setDeleteRow] = useState<ThreadRow | null>(null);
  const [keyError, setKeyError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  // `j` on the last loaded row asks for more rows and waits for them.
  const pendingMove = useRef<{
    afterId: number;
    count: number;
    scope: string;
  } | null>(null);
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const [compose, setCompose] = useState<{
    key: number;
    request: ComposeRequest;
  } | null>(null);
  const outbox = useMailOutbox();
  const [outboxOpen, setOutboxOpen] = useState(false);
  const unconfirmed =
    outbox.rows?.filter((r) => r.state === "unconfirmed").length ?? 0;
  const total = outbox.rows?.length ?? 0;
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
  const archived = folderPref === ARCHIVE_PREF;
  const folderPending =
    prefFolderId !== null && folders === null && foldersError === null;
  const mailboxAddress = mailboxes.find((m) => m.id === mailboxId)?.address;
  const scope: ListBase | null = folderPending
    ? null
    : archived
      ? { kind: "archived" }
      : folderId !== null
        ? { kind: "folder", folderId }
        : mailboxId !== null && mailboxAddress
          ? { kind: "mailbox", mailboxId, address: mailboxAddress }
          : { kind: "all" };
  const list = useMailThreadList(scope);
  const scopeKey = JSON.stringify(scope);
  const pick = (mailbox: string | null, folder: string | null) => {
    selectedMailbox.save(mailbox);
    selectedFolder.save(folder);
    setThreadId(null);
    setCursorId(null);
    pendingMove.current = null;
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
  // With no reading pane an opened thread takes the list's place: otherwise
  // Enter, or a click, would open something nowhere.
  const readerOnly = side === "off" && threadId !== null;

  const onKey = (event: KeyboardEvent) => {
    // A dialog or an open menu anywhere (the composer, a delete confirmation,
    // the rail's context menu) owns the keyboard, even when focus has slipped
    // to the body: `e` must not archive the mail behind it.
    if (document.querySelector(OPEN_SURFACE)) return;
    const action = mailKeyAction(event);
    if (action) runKey(action, event);
  };
  const runKey = (action: MailKeyAction, event: KeyboardEvent) => {
    const rows = list.rows;
    const at = rows.findIndex((row) => row.id === cursorId);
    const row =
      rows[at] ?? rows.find((candidate) => candidate.id === threadId) ?? null;
    const targetId = row?.id ?? threadId;
    const handled = () => event.preventDefault();
    // Any key is a newer intent than a move still waiting on a page.
    pendingMove.current = null;

    if (action === "next" || action === "prev") {
      if (readerOnly || rows.length === 0) return;
      const to = at === -1 ? 0 : at + (action === "next" ? 1 : -1);
      if (to < 0) return;
      // Past the loaded rows this fetches the next page, and the move waits
      // for it (see the pendingMove effect).
      if (to >= list.shown) list.grow();
      const next = rows[to];
      if (next) setCursorId(next.id);
      else if (action === "next" && at >= 0 && list.hasMore) {
        pendingMove.current = {
          afterId: rows[at]!.id,
          count: rows.length,
          scope: scopeKey,
        };
      }
      handled();
    } else if (action === "open") {
      if (!row) return;
      setThreadId(row.id);
      handled();
    } else if (action === "back") {
      if (!readerOnly) return;
      setThreadId(null);
      handled();
    } else if (action === "search") {
      const box = rootRef.current?.querySelector<HTMLInputElement>(
        'input[type="search"]',
      );
      // The Archive and smart folders have no search box; leave "/" alone.
      if (!box) return;
      box.focus();
      box.select();
      handled();
    } else if (action === "reply" || action === "reply_all") {
      if (targetId === null) return;
      handled();
      setKeyError(null);
      mailThread(targetId)
        .then((detail) => {
          const sorted = sortedMessages(detail.messages);
          const message = sorted[sorted.length - 1];
          if (message) {
            openCompose({ kind: action, message, thread: detail.thread });
          }
        })
        .catch((err) => setKeyError(errorText(err)));
    } else if (action === "archive") {
      if (targetId === null || scope?.kind === "archived") return;
      handled();
      setKeyError(null);
      // From the target, not the cursor: with no cursor the target is the open
      // thread, and `at` of -1 would pick the first row, possibly that thread.
      const index = rows.findIndex((r) => r.id === targetId);
      const neighbour =
        index < 0 ? null : (rows[index + 1] ?? rows[index - 1] ?? null);
      mailDelete([targetId], false)
        .then((report) => {
          // An archive can leave attachment files behind; say so, as the
          // dialog does, rather than looking clean.
          setKeyError(archiveNotice(report));
          setCursorId(neighbour?.id ?? null);
          if (threadId === targetId) setThreadId(null);
          list.reload();
        })
        .catch((err) => setKeyError(errorText(err)));
    } else if (action === "delete") {
      if (!row) return;
      handled();
      setDeleteRow(row);
    }
  };
  // Applies a `j` that ran off the loaded rows once the grown rows land.
  useEffect(() => {
    const pending = pendingMove.current;
    if (!pending) return;
    // Dropped if the scope changed, or the fetch failed, or the rows shrank.
    if (pending.scope !== scopeKey || list.error !== null) {
      pendingMove.current = null;
      return;
    }
    if (list.rows.length <= pending.count) return;
    pendingMove.current = null;
    const from = list.rows.findIndex((r) => r.id === pending.afterId);
    const next = from < 0 ? undefined : list.rows[from + 1];
    if (next) setCursorId(next.id);
  }, [list.rows, list.error, scopeKey]);

  // Re-bound through a ref so the one listener always sees this render's list
  // and selection without being torn down on every keystroke.
  const onKeyRef = useRef(onKey);
  onKeyRef.current = onKey;
  useEffect(() => {
    // Mounted only while Mail is the open view, which is what scopes these
    // keys: the app mounts no MailView under any other view.
    const listener = (event: KeyboardEvent) => onKeyRef.current(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  // j/k move real focus, so Tab, Enter and a screen reader follow the cursor.
  useEffect(() => {
    if (cursorId === null || readerOnly) return;
    rootRef.current
      ?.querySelector<HTMLElement>(`[data-thread-id="${cursorId}"]`)
      ?.focus();
  }, [cursorId, readerOnly]);

  const actionButtons = (
    <div className="flex shrink-0 items-center gap-1.5 pr-2">
      <SecondaryButton
        danger={unconfirmed > 0}
        aria-label={outboxLabel(total, unconfirmed)}
        onClick={() => setOutboxOpen(true)}
      >
        <Send className="size-3" />
        Outbox
        {total > 0 ? (
          <span className="tabular-nums text-content/50">{total}</span>
        ) : null}
        {unconfirmed > 0 ? (
          <span className="tabular-nums">{unconfirmed} to check</span>
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
  );

  return (
    <div
      ref={rootRef}
      role="region"
      aria-label="Mail"
      data-app-mail
      className="flex min-h-0 min-w-0 flex-1 flex-col text-content"
    >
      {tabbed ? (
        <div className="flex h-10 shrink-0 items-center justify-end border-b border-stroke">
          {actionButtons}
        </div>
      ) : (
        <div
          data-app-view-header
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
          {actionButtons}
          {IS_MAC ? null : <WindowControls />}
        </div>
      )}
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
            archiveSelected={archived}
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
            onSelectArchive={() => pick(null, ARCHIVE_PREF)}
            onSynced={() => void refresh()}
            progress={progress}
          />
        </aside>
        <div
          className={`flex min-h-0 min-w-0 flex-1 ${
            side === "bottom" ? "flex-col" : ""
          }`}
        >
          {readerOnly ? null : (
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
              <MailSourceErrors status={status} />
              {keyError ? (
                <p
                  role="alert"
                  className="shrink-0 border-b border-stroke px-3 py-2 text-[12px] text-red-400/90"
                >
                  {keyError}
                </p>
              ) : null}
              <MailThreadList
                list={list}
                scope={scope ?? { kind: "all" }}
                mailboxes={mailboxes}
                selectedId={threadId}
                cursorId={cursorId}
                setup={sourceSetup(status)}
                onSelect={(thread) => {
                  setThreadId(thread.id);
                  setCursorId(thread.id);
                }}
                onSaveAsFolder={(rules, matchMode) =>
                  setSheet({ kind: "new", rules, matchMode })
                }
                onEditFolder={() => {
                  const folder = folders?.find((f) => f.id === folderId);
                  if (folder) setSheet({ kind: "edit", folder });
                }}
              />
            </section>
          )}
          {side === "off" && !readerOnly ? null : (
            <section
              aria-label="Reading pane"
              data-mail-pane="reader"
              className="flex min-h-0 min-w-0 flex-1 flex-col"
            >
              {readerOnly ? (
                <div className="flex shrink-0 items-center border-b border-stroke px-3 py-1.5">
                  <SecondaryButton onClick={() => setThreadId(null)}>
                    <ChevronLeft className="size-3" />
                    Messages
                  </SecondaryButton>
                </div>
              ) : null}
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
            // A save emits nothing, so this is when the list learns of it. A
            // draft saved by the composer in another window is not seen until
            // the next reload.
            void outbox.reload();
          }}
        />
      ) : null}
      {deleteRow ? (
        <DeleteMailDialog
          threadIds={[deleteRow.id]}
          subject={deleteRow.subject}
          messageCount={deleteRow.messageCount}
          mailbox={deleteRow.mailboxAddress}
          alreadyArchived={archived}
          onClose={() => setDeleteRow(null)}
          onDone={(report, hard) => {
            list.reload();
            // Gone for good only when S3 confirmed every message; a partial
            // delete leaves the thread there to be read and retried.
            if (hard && report.kept === 0 && report.errors.length === 0) {
              if (threadId === deleteRow.id) setThreadId(null);
              setCursorId(null);
            }
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
