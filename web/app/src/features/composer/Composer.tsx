// F-47, F-48: the prompt field of a session and of the new-session page. A glass card: an optional top row
// (the workspace menu), the attachment strip, an auto-growing textarea (64 to 160 px), and a bottom row with
// the add button, the toolbar (model controls) and the round Send or Stop button.
//
// - Send: the button always; Enter only on a desktop layout (a wide screen with a fine pointer), where
//   Shift+Enter is a newline. Ctrl or Cmd+Enter sends everywhere. A draft mid-turn still sends (the desktop
//   steers or queues); Stop shows when the draft is empty.
// - The parent's `onSubmit` does the guard and the request. It resolves when the message is on its way (the
//   draft is cleared) and rejects to keep the draft; the rejection's message is shown (a `SendCancelled` is silent).
// - A read-only device cannot write: the field and buttons are disabled and a line says why.
// - Offline (the link is not ready) keeps the field open for writing; only Send and Stop wait.
// - Attachments: a file picker, paste and drop, limited as in `attachments.ts`.
// - F-47 morph (design-system.md section 6.2 and the Composer row): at rest the glass is a one-line pill, 50 px
//   high with 25 px corners: add, dictation, the field and Send in one row; the workspace row and the model
//   controls are clipped to nothing (still in the DOM, so a Tab into them focuses and opens the card). It becomes
//   the card (26 px corners, field min 64, two rows) while it has focus, a draft, an attachment, a notice or an
//   error. The size change is a `motion` layout animation of 280 ms out-cubic (`--ease-out-cubic`); motion writes
//   through the CSSOM, so the CSP needs no inline style attribute. With `prefers-reduced-motion` it is instant.
//   `data-shape` says which one it is.
import { useReducedMotion } from "motion/react";
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ClipboardEvent, type DragEvent, type KeyboardEvent, type ReactNode } from "react";
import { DictationButton } from "@/features/dictation";
import { cn } from "@/lib/utils";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { useMediaQuery } from "@/ui/useMediaQuery";
import {
  AttachmentError,
  attachmentFromFile,
  attachmentRoom,
  filesFromClipboard,
  mergeAttachments,
  revokeAttachment,
  type ComposerAttachment,
} from "./attachments";
import { AttachmentChip } from "./AttachmentChip";
import { useKeyboardInset } from "./useKeyboardInset";
import { SendCancelled } from "./useSendGuard";

export const COMPOSER_PLACEHOLDER = "Plan, ask, build...";
export const BUSY_PLACEHOLDER = "Send a follow-up...";
export const OFFLINE_PLACEHOLDER = "Reconnecting to desktop...";
export const READ_ONLY_REASON = "Read-only: this device can read sessions but not send messages.";
/** A wide screen with a mouse or trackpad: Enter sends there, and only there. */
export const DESKTOP_LAYOUT_QUERY = "(min-width: 640px) and (pointer: fine)";
const INPUT_MIN_PX = 64;
const INPUT_MAX_PX = 160;
/** The resting pill: a one-line field with fully round ends. */
export const PILL_FIELD_MIN_PX = 24;
export const PILL_HEIGHT_PX = 50;
export const PILL_RADIUS_PX = 25;
export const CARD_RADIUS_PX = 26;
/** design-system.md section 6.2: 280 ms with `--ease-out-cubic` (cubic-bezier(0.215, 0.61, 0.355, 1)). */
export const MORPH_TRANSITION = { duration: 0.28, ease: [0.215, 0.61, 0.355, 1] } as const;

export interface ComposerProps {
  /** Guards and sends. Resolve to clear the draft, reject to keep it. */
  onSubmit: (text: string, attachments: ComposerAttachment[]) => Promise<void>;
  /** A turn is running: the placeholder offers a follow-up and Stop shows for an empty draft. */
  busy?: boolean;
  onStop?: () => Promise<void>;
  readOnly?: boolean;
  /** The link is not ready: Send and Stop wait, writing does not. */
  offline?: boolean;
  /** Replaces the field with a line (the session's latest error) until dismissed. */
  notice?: { text: string; dismiss: () => void } | null;
  /** Model controls and the like, between the add button and Send. */
  toolbar?: ReactNode;
  /** Above the field, inside the card (the workspace menu). */
  topRow?: ReactNode;
  autoFocus?: boolean;
  className?: string;
}

export function Composer({ onSubmit, busy = false, onStop, readOnly = false, offline = false, notice, toolbar, topRow, autoFocus = false, className }: ComposerProps) {
  const [text, setText] = useState("");
  const [attachments, setAttachmentsState] = useState<ComposerAttachment[]>([]);
  // Read by the async picks, which must not merge into a stale list.
  const held = useRef<ComposerAttachment[]>([]);
  const setAttachments = (next: ComposerAttachment[]) => {
    held.current = next;
    setAttachmentsState(next);
  };
  const [submitting, setSubmitting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const reasonId = useId();
  const errorId = useId();
  const desktopLayout = useMediaQuery(DESKTOP_LAYOUT_QUERY);
  const inset = useKeyboardInset();
  const blocked = !!notice;
  const reducedMotion = useReducedMotion();
  const [focusWithin, setFocusWithin] = useState(false);

  const hasDraft = text.trim().length > 0 || attachments.length > 0;
  const card = focusWithin || hasDraft || blocked || !!error;
  const fieldMin = card ? INPUT_MIN_PX : PILL_FIELD_MIN_PX;
  const canSend = hasDraft && !submitting && !readOnly && !offline && !blocked;
  const showStop = !hasDraft && busy && !!onStop;

  // Auto-grow: as tall as the text, between 64 and 160 px.
  useLayoutEffect(() => {
    const element = area.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${Math.min(INPUT_MAX_PX, Math.max(fieldMin, element.scrollHeight))}px`;
  }, [text, fieldMin]);

  useEffect(() => {
    if (autoFocus && !readOnly) area.current?.focus();
  }, [autoFocus, readOnly]);

  // iOS scrolls the page to the focused field; the layout is a fixed shell, so put the page back.
  const scrollTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(scrollTimer.current), []);
  const onFocus = () => {
    window.clearTimeout(scrollTimer.current);
    scrollTimer.current = window.setTimeout(() => window.scrollTo(0, 0), 50);
  };

  const attach = async (files: File[]) => {
    if (files.length === 0 || readOnly) return;
    const room = attachmentRoom(held.current);
    if (room <= 0) {
      setError("A message can carry up to 10 files.");
      return;
    }
    setError(null);
    let picked: ComposerAttachment[] = [];
    try {
      // Settle every file first: when one fails, the previews of the others are still revoked.
      const settled = await Promise.allSettled(files.slice(0, room).map(attachmentFromFile));
      picked = settled.flatMap((result) => (result.status === "fulfilled" ? [result.value] : []));
      const failed = settled.find((result) => result.status === "rejected");
      if (failed) throw (failed as PromiseRejectedResult).reason;
      setAttachments(mergeAttachments(held.current, picked));
      area.current?.focus();
    } catch (reason) {
      picked.forEach(revokeAttachment);
      setError(reason instanceof AttachmentError ? reason.message : "Could not attach.");
    }
  };

  // F-49: a finished phrase joins the draft after what is already typed; it never sends.
  const insertText = useCallback((phrase: string) => {
    setText((current) => (current.length === 0 || /\s$/.test(current) ? current + phrase : `${current} ${phrase}`));
  }, []);

  const remove = (id: string) => {
    held.current.filter((item) => item.id === id).forEach(revokeAttachment);
    setAttachments(held.current.filter((item) => item.id !== id));
  };

  const submit = async () => {
    if (!canSend) return;
    const message = text.trim();
    const files = attachments;
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit(message, files);
      // Text typed while the send was in flight (or in the send-guard dialog) belongs to the next message.
      setText((current) => (current.trim() === message ? "" : current));
      setAttachments([]);
      files.forEach(revokeAttachment);
    } catch (reason) {
      if (!(reason instanceof SendCancelled)) setError(reason instanceof Error && reason.message ? reason.message : "Could not send the message.");
    } finally {
      setSubmitting(false);
    }
  };

  const stop = async () => {
    if (!onStop || stopping || offline || readOnly) return;
    setStopping(true);
    setError(null);
    try {
      await onStop();
    } catch (reason) {
      setError(reason instanceof Error && reason.message ? `Could not stop: ${reason.message}` : "Could not stop the session.");
    } finally {
      setStopping(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.nativeEvent.isComposing || event.keyCode === 229) return;
    const modified = event.metaKey || event.ctrlKey;
    if (modified || (desktopLayout && !event.shiftKey)) {
      event.preventDefault();
      void submit();
    }
  };

  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = filesFromClipboard(event.clipboardData);
    if (files.length === 0) return;
    event.preventDefault();
    void attach(files);
  };

  const onDrop = (event: DragEvent<HTMLElement>) => {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    void attach(Array.from(event.dataTransfer.files));
  };

  const placeholder = offline ? OFFLINE_PLACEHOLDER : busy ? BUSY_PLACEHOLDER : COMPOSER_PLACEHOLDER;
  const describedBy = [readOnly ? reasonId : null, error ? errorId : null].filter(Boolean).join(" ") || undefined;

  return (
    <div data-testid="composer" className={cn("pb-3 pt-1", className)} style={inset ? { marginBottom: inset } : undefined}>
      <Glass
        className={cn("flex", card ? "flex-col gap-1 px-3 pb-2 pt-3" : "min-h-[50px] flex-row items-center gap-1 px-1.5 py-[3px]")}
        data-shape={card ? "card" : "pill"}
        layout
        initial={false}
        animate={{ borderRadius: card ? CARD_RADIUS_PX : PILL_RADIUS_PX }}
        transition={reducedMotion ? { duration: 0 } : MORPH_TRANSITION}
        onFocus={() => setFocusWithin(true)}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocusWithin(false);
        }}
        onDragOver={(event) => {
          if (event.dataTransfer.types.includes("Files")) event.preventDefault();
        }}
        onDrop={onDrop}
      >
        {topRow ? <div className={card ? "flex min-w-0 items-center gap-2 px-1" : "size-0 overflow-hidden"}>{topRow}</div> : null}

        {attachments.length > 0 ? (
          <ul aria-label="Attachments" className="m-0 flex list-none gap-2 overflow-x-auto px-1 py-1">
            {attachments.map((attachment) => (
              <li key={attachment.id} className="shrink-0">
                <AttachmentChip attachment={attachment} onRemove={() => remove(attachment.id)} />
              </li>
            ))}
          </ul>
        ) : null}

        {blocked && notice ? (
          <button
            type="button"
            onClick={notice.dismiss}
            aria-label={`Couldn't continue: ${notice.text}`}
            className="flex min-h-11 w-full items-center gap-2 rounded-row px-2 text-left text-small text-deletions hover:bg-selection"
          >
            <span className="min-w-0 flex-1 truncate">{notice.text}</span>
            <Icon name="close" size={14} className="shrink-0 text-muted-text" />
          </button>
        ) : (
          <textarea
            ref={area}
            value={text}
            rows={1}
            disabled={readOnly}
            aria-label="Message"
            aria-describedby={describedBy}
            enterKeyHint={desktopLayout ? "send" : "enter"}
            placeholder={placeholder}
            onChange={(event) => setText(event.target.value)}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            onFocus={onFocus}
            className={cn("order-3 block min-w-0 resize-none bg-transparent text-row text-content outline-none placeholder:text-muted-text disabled:opacity-60", card ? "order-none w-full px-1" : "flex-1 px-2")}
            style={{ minHeight: fieldMin, maxHeight: INPUT_MAX_PX }}
          />
        )}

        <div className={card ? cn("flex min-w-0 items-center gap-2", blocked && "pointer-events-none opacity-50") : "contents"}>
          <input
            ref={picker}
            type="file"
            multiple
            hidden
            data-testid="composer-file-input"
            onChange={(event) => {
              const files = Array.from(event.target.files ?? []);
              event.target.value = "";
              void attach(files);
            }}
          />
          <button
            type="button"
            aria-label="Add photos or files"
            disabled={readOnly || blocked}
            onClick={() => picker.current?.click()}
            className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection disabled:opacity-40 pointer-fine:size-9", !card && "order-1")}
          >
            <Icon name="add" size={18} />
          </button>
          <DictationButton onText={insertText} disabled={readOnly || blocked} className={cn("size-11 pointer-fine:size-9", !card && "order-2")} />
          <div className={card ? "flex min-w-0 flex-1 items-center" : "order-4 size-0 overflow-hidden"}>{toolbar}</div>
          {showStop ? (
            <button
              type="button"
              aria-label="Stop"
              disabled={stopping || offline || readOnly}
              onClick={() => void stop()}
              className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-primary text-primary-foreground disabled:opacity-40 pointer-fine:size-9", !card && "order-5")}
            >
              <span aria-hidden="true" className="block size-3 rounded-[3px] bg-current" />
            </button>
          ) : (
            <button
              type="button"
              aria-label="Send"
              disabled={!canSend}
              aria-busy={submitting}
              onClick={() => void submit()}
              className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-primary text-primary-foreground disabled:opacity-40 pointer-fine:size-9", !card && "order-5")}
            >
              <Icon name="send" size={18} />
            </button>
          )}
        </div>
      </Glass>
      {readOnly ? (
        <p id={reasonId} className="px-4 pt-2 text-meta text-muted-text">
          {READ_ONLY_REASON}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} role="alert" className="px-4 pt-2 text-meta text-deletions">
          {error}
        </p>
      ) : null}
    </div>
  );
}
