import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ChevronDown,
  ChevronRight,
  Loader,
  Paperclip,
  Reply,
  ReplyAll,
  Send,
  Star,
} from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import {
  MAIL_MESSAGES_CHANGED_EVENT,
  mailAttachmentOpen,
  mailMark,
  mailMessageRaw,
  mailStar,
  mailThread,
} from "../model/mail";
import type { ComposeRequest } from "../model/mailReply";
import type {
  AttachmentRow,
  MailAddress,
  MessageRow,
  ThreadDetail,
} from "../model/mailTypes";
import { preloadMailEditor } from "./ComposeSheet";
import { DeleteMailDialog } from "./DeleteMailDialog";
import { MailMessageBody, canTogglePlain } from "./MailMessageBody";
import { formatMailDate } from "./MailThreadList";

/**
 * Senders the user chose to always load images from. There is no command to
 * write a per-sender policy (only the per-source `image_policy` exists), so
 * this lasts until the app restarts.
 */
const allowedSenders = new Set<string>();

const addressOf = (p: MailAddress) =>
  p.name ? `${p.name} <${p.addr}>` : p.addr;
const senderOf = (m: MessageRow) => m.fromName?.trim() || m.fromAddr;

/** `dateUtc` is epoch seconds. */
const fullDate = (seconds: number) => new Date(seconds * 1000).toLocaleString();

const sizeLabel = (bytes: number) =>
  bytes < 1024
    ? `${bytes} B`
    : bytes < 1024 * 1024
      ? `${Math.round(bytes / 1024)} KB`
      : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;

/** The verdict as the receiving server wrote it; NULL means `none`. */
function Verdict({ label, value }: { label: string; value: string | null }) {
  const tone =
    value === "fail"
      ? "text-red-400"
      : // permerror is a broken record on the sender's side, not a forgery signal.
        value === "softfail" || value === "permerror"
        ? "text-amber-400"
        : "text-content/60";
  return (
    <span className="rounded-full border border-content/10 bg-content/5 px-1.5 text-[10px]">
      {label} <span className={tone}>{value ?? "none"}</span>
    </span>
  );
}

function AttachmentChip({ attachment }: { attachment: AttachmentRow }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const open = async () => {
    setBusy(true);
    setError(null);
    try {
      // Reveal rather than launch: a received file should not run on one click.
      await revealItemInDir(await mailAttachmentOpen(attachment.id));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex flex-col gap-0.5">
      <button
        type="button"
        disabled={busy}
        onClick={() => void open()}
        title="Download and show in folder"
        className="flex items-center gap-1.5 rounded-md border border-content/10 px-2 py-1 text-[12px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40"
      >
        {busy ? (
          <Loader className="size-3 animate-spin" />
        ) : (
          <Paperclip className="size-3" />
        )}
        <span className="max-w-[240px] truncate">{attachment.filename}</span>
        <span className="text-content/45">
          {sizeLabel(attachment.sizeBytes)}
        </span>
      </button>
      {error ? <span className="text-[11px] text-red-400">{error}</span> : null}
    </div>
  );
}

/** The stored message as received, for when the sanitiser removed something. */
/**
 * The stored source can be tens of MB. Laying all of it out would freeze the
 * app, and the start is what shows what the sanitiser removed.
 */
export const RAW_PREVIEW_CHARS = 512 * 1024;

function RawSource({
  messageId,
  onClose,
}: {
  messageId: number;
  onClose: () => void;
}) {
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    mailMessageRaw(messageId)
      .then((text) => {
        if (!stale) setRaw(text);
      })
      .catch((err) => {
        if (!stale) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      stale = true;
    };
  }, [messageId]);
  return (
    <Modal title="Message source" size="md" onClose={onClose}>
      {error ? (
        <p className="px-4 py-3 text-[12px] text-red-400">{error}</p>
      ) : raw === null ? (
        <div className="flex justify-center py-6 text-content/45">
          <Loader className="size-4 animate-spin" />
        </div>
      ) : (
        // Sender-controlled text, shown as a text node and never parsed.
        <>
          {raw.length > RAW_PREVIEW_CHARS ? (
            <p className="px-4 pt-3 text-[12px] text-content/50">
              Truncated: showing the first 512 KB of{" "}
              {(raw.length / (1024 * 1024)).toFixed(1)} MB.
            </p>
          ) : null}
          <pre className="max-h-[70vh] overflow-auto whitespace-pre-wrap break-words px-4 py-3 font-mono text-[11px] text-content/80">
            {raw.slice(0, RAW_PREVIEW_CHARS)}
          </pre>
        </>
      )}
    </Modal>
  );
}

function MessageCard({
  message,
  detail,
  expanded,
  onToggle,
  senderAllowed,
  onAllowSender,
  messageAllowed,
  onAllowMessage,
}: {
  message: MessageRow;
  detail: ThreadDetail;
  expanded: boolean;
  onToggle: () => void;
  senderAllowed: boolean;
  onAllowSender: () => void;
  messageAllowed: boolean;
  onAllowMessage: () => void;
}) {
  const [plain, setPlain] = useState(false);
  const [source, setSource] = useState(false);
  const attachments = message.attachments.filter((a) => !a.isInline);
  const Chevron = expanded ? ChevronDown : ChevronRight;

  return (
    <article
      aria-label={`Message from ${senderOf(message)}`}
      className="border-b border-stroke"
    >
      <button
        type="button"
        aria-expanded={expanded}
        onClick={onToggle}
        className="flex w-full items-center gap-2 px-4 py-2 text-left text-[12px] hover:bg-content/5 focus-visible:outline-2 focus-visible:outline-accent"
      >
        <Chevron className="size-3 shrink-0 text-content/45" />
        <span
          className={`shrink-0 ${
            message.isRead ? "text-content/70" : "font-semibold text-content"
          }`}
        >
          {senderOf(message)}
        </span>
        {expanded ? null : (
          // Sender-controlled text: rendered as a React text node, never as HTML.
          <span className="min-w-0 flex-1 truncate text-content/45">
            {message.snippet}
          </span>
        )}
        <span className="ml-auto shrink-0 text-content/45">
          {expanded
            ? fullDate(message.dateUtc)
            : formatMailDate(message.dateUtc)}
        </span>
      </button>
      {expanded ? (
        <>
          <div className="flex flex-col gap-1 px-4 pb-2 text-[12px] text-content/50">
            <div>
              {addressOf({ name: message.fromName, addr: message.fromAddr })}
            </div>
            {message.to.length > 0 ? (
              <div>To: {message.to.map(addressOf).join(", ")}</div>
            ) : null}
            {message.cc.length > 0 ? (
              <div>Cc: {message.cc.map(addressOf).join(", ")}</div>
            ) : null}
            {message.bcc.length > 0 ? (
              <div>Bcc: {message.bcc.map(addressOf).join(", ")}</div>
            ) : null}
            <div className="flex flex-wrap items-center gap-1.5">
              {message.direction === "inbound" ? (
                <span
                  className="flex gap-1.5"
                  title="As reported by the receiving mail server. This does not prove who sent the message."
                >
                  <Verdict label="SPF" value={message.spf} />
                  <Verdict label="DKIM" value={message.dkim} />
                  <Verdict label="DMARC" value={message.dmarc} />
                </span>
              ) : null}
              {canTogglePlain(message) ? (
                <button
                  type="button"
                  aria-pressed={plain}
                  onClick={() => setPlain((p) => !p)}
                  className="rounded-full border border-content/10 px-1.5 text-[10px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
                >
                  Plain text
                </button>
              ) : null}
              <button
                type="button"
                onClick={() => setSource(true)}
                className="rounded-full border border-content/10 px-1.5 text-[10px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
              >
                View source
              </button>
              {source ? (
                <RawSource
                  messageId={message.id}
                  onClose={() => setSource(false)}
                />
              ) : null}
            </div>
          </div>
          <MailMessageBody
            message={message}
            plain={plain}
            imagePolicy={detail.imagePolicy}
            senderAllowed={senderAllowed}
            senderLabel={message.fromAddr}
            onAllowSender={onAllowSender}
            messageAllowed={messageAllowed}
            onAllowMessage={onAllowMessage}
          />
          {attachments.length > 0 ? (
            <div className="flex flex-wrap gap-2 px-4 py-3">
              {attachments.map((a) => (
                <AttachmentChip key={a.id} attachment={a} />
              ))}
            </div>
          ) : null}
        </>
      ) : null}
    </article>
  );
}

type ComposeFromThread = (
  request: Extract<ComposeRequest, { message: MessageRow }>,
) => void;

function ThreadBody({
  threadId,
  onCompose,
}: {
  threadId: number;
  onCompose?: ComposeFromThread;
}) {
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  // Only the user's changes from the default (newest open, rest closed) are kept.
  const [flipped, setFlipped] = useState<ReadonlySet<number>>(new Set());
  const [, bump] = useState(0);
  // Kept here, not in the card, so collapsing a message keeps its images.
  const [imagesOk, setImagesOk] = useState<ReadonlySet<number>>(new Set());
  const markedOpen = useRef(false);
  const [deleting, setDeleting] = useState(false);
  // Set once S3 confirmed every message gone, so the pane stops reloading a
  // thread that no longer exists.
  const [deleted, setDeleted] = useState(false);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen(MAIL_MESSAGES_CHANGED_EVENT, () => setReloadKey((k) => k + 1))
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, []);

  useEffect(() => {
    if (deleted) return;
    let stale = false;
    mailThread(threadId)
      .then((next) => {
        if (stale) return;
        setDetail(next);
        setError(null);
      })
      .catch((err) => {
        if (!stale) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      stale = true;
    };
  }, [threadId, reloadKey, deleted]);

  // Opening a thread reads it. Once per open, so "Mark as unread" stays undone.
  useEffect(() => {
    if (!detail || markedOpen.current) return;
    markedOpen.current = true;
    if (detail.messages.some((m) => !m.isRead)) {
      void mailMark([threadId], true).catch(() => undefined);
    }
  }, [detail, threadId]);

  if (deleted) {
    return (
      <p className="px-4 py-3 text-[12px] text-content/50">
        This conversation was deleted.
      </p>
    );
  }
  if (error && !detail) {
    return <p className="px-4 py-3 text-[12px] text-red-400">{error}</p>;
  }
  if (!detail) {
    return (
      <div className="flex flex-1 items-center justify-center text-content/45">
        <Loader className="size-4 animate-spin" />
      </div>
    );
  }

  // Oldest first, as a conversation reads. The array order is not relied on.
  const messages = [...detail.messages].sort(
    (a, b) => a.dateUtc - b.dateUtc || a.id - b.id,
  );
  const newest = messages[messages.length - 1];
  const newestId = newest?.id;
  const unread = messages.some((m) => !m.isRead);
  const { thread } = detail;

  const run = async (action: () => Promise<void>) => {
    try {
      // mail_mark and mail_star emit the change event; the listener reloads.
      await action();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <>
      <header className="flex items-start gap-3 border-b border-stroke px-4 py-3">
        <h2 className="min-w-0 flex-1 break-words text-[15px] font-medium text-content">
          {thread.subject || "(no subject)"}
        </h2>
        <button
          type="button"
          aria-label={thread.isStarred ? "Remove star" : "Star"}
          aria-pressed={thread.isStarred}
          title={thread.isStarred ? "Remove star" : "Star"}
          onClick={() =>
            void run(() => mailStar([thread.id], !thread.isStarred))
          }
          className={`rounded-md p-1 hover:bg-content/10 focus-visible:outline-2 focus-visible:outline-accent ${
            thread.isStarred ? "text-amber-400" : "text-content/45"
          }`}
        >
          <Star className="size-4" />
        </button>
        {onCompose && newest ? (
          <>
            {(
              [
                ["reply", "Reply", Reply],
                ["reply_all", "Reply all", ReplyAll],
                ["forward", "Forward", Send],
              ] as const
            ).map(([kind, label, Icon]) => (
              <SecondaryButton
                key={kind}
                onPointerEnter={preloadMailEditor}
                onFocus={preloadMailEditor}
                onClick={() => onCompose({ kind, message: newest, thread })}
              >
                <Icon className="size-3" />
                {label}
              </SecondaryButton>
            ))}
          </>
        ) : null}
        <SecondaryButton
          onClick={() => void run(() => mailMark([thread.id], unread))}
        >
          {unread ? "Mark as read" : "Mark as unread"}
        </SecondaryButton>
        <SecondaryButton danger onClick={() => setDeleting(true)}>
          Delete
        </SecondaryButton>
      </header>
      {deleting ? (
        <DeleteMailDialog
          threadIds={[thread.id]}
          subject={thread.subject}
          messageCount={messages.length}
          mailbox={thread.mailboxAddress}
          onClose={() => setDeleting(false)}
          onDone={(report, hard) => {
            if (hard && report.kept === 0 && report.errors.length === 0) {
              setDeleted(true);
            }
          }}
        />
      ) : null}
      {error ? (
        <p className="px-4 py-2 text-[12px] text-red-400">{error}</p>
      ) : null}
      {messages.map((message) => {
        const sender = message.fromAddr.toLowerCase();
        return (
          <MessageCard
            key={message.id}
            message={message}
            detail={detail}
            expanded={(message.id === newestId) !== flipped.has(message.id)}
            onToggle={() =>
              setFlipped((prev) => {
                const next = new Set(prev);
                if (!next.delete(message.id)) next.add(message.id);
                return next;
              })
            }
            senderAllowed={allowedSenders.has(sender)}
            messageAllowed={imagesOk.has(message.id)}
            onAllowMessage={() =>
              setImagesOk((prev) => new Set(prev).add(message.id))
            }
            onAllowSender={() => {
              allowedSenders.add(sender);
              bump((n) => n + 1);
            }}
          />
        );
      })}
    </>
  );
}

/**
 * The reading pane. State lives here, outside MailView's keyed threads
 * section, so a layout change that remounts the list does not collapse the
 * messages the user opened.
 */
export function MailThreadRead({
  threadId,
  onCompose,
}: {
  threadId: number | null;
  onCompose?: ComposeFromThread;
}) {
  if (threadId === null) {
    return (
      <div className="flex flex-1 items-center justify-center text-[12px] text-content/40">
        Select a message to read it.
      </div>
    );
  }
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <ThreadBody key={threadId} threadId={threadId} onCompose={onCompose} />
    </div>
  );
}
