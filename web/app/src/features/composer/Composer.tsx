// F-47, F-48: the prompt field of a session and of the new-session page. A glass card: an optional top row
// (the workspace menu), the attachment strip, an auto-growing textarea (64 to 160 px), and a bottom row with
// the add button, the toolbar (model controls) and the round Send or Stop button.
//
// - Send: the button always; Enter only on a desktop layout (a wide screen with a fine pointer), where
//   Shift+Enter is a newline. Ctrl or Cmd+Enter sends everywhere. A draft mid-turn still sends (the desktop
//   steers or queues). Stop shows for the whole turn: alone when the draft is empty, left of Send when there is a
//   draft. It is armed STOP_ARM_DELAY_MS after it appears and again after every slot change (a tap aimed at Send
//   cannot land on it) and says "Stopping..." until the turn ends. If the turn is still running STOP_RETRY_MS
//   after the cancel request resolved, Stop can be pressed again.
// - The parent's `onSubmit` does the guard and the request. It resolves when the message is on its way (the
//   draft is cleared) and rejects to keep the draft; the rejection's message is shown (a `SendCancelled` is silent).
// - A read-only device cannot write: the field and buttons are disabled and a line says why, with a "Request
//   control" action when the page passes `requestControl`.
// - The session's latest error is a compact notice row inside the card; the field and toolbar stay usable.
// - Offline (the link is not ready) keeps the field open for writing; only Send and Stop wait.
// - Attachments: a file picker, paste and drop, limited as in `attachments.ts`.
// - F-47 morph (design-system.md section 6.2 and the Composer row): at rest the glass is a one-line pill, 50 px
//   high with 25 px corners: add, dictation, the field and Send in one row; the workspace row and the model
//   controls are clipped to nothing (still in the DOM, so a Tab into them focuses and opens the card). It becomes
//   the card (26 px corners, field min 64, two rows) while the field or a keyboard-focused control has focus, a draft, an attachment, a notice or an
//   error. The size change is a `motion` layout animation of 280 ms out-cubic (`--ease-out-cubic`); motion writes
//   through the CSSOM, so the CSP needs no inline style attribute. With `prefers-reduced-motion` it is instant.
//   `data-shape` says which one it is.
import { useReducedMotion } from "motion/react";
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ClipboardEvent, type DragEvent, type KeyboardEvent, type ReactNode } from "react";
import { DictationButton } from "@/features/dictation";
import { cn } from "@/lib/utils";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { sessionErrorContent } from "@/features/transcript/sessionError";
import { SessionErrorDialog } from "@/features/transcript/TurnDialogs";
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
import type { RequestControl } from "./useRequestControl";
import { SendCancelled } from "./useSendGuard";

export const COMPOSER_PLACEHOLDER = "Plan, ask, build...";
export const BUSY_PLACEHOLDER = "Send a follow-up...";
export const OFFLINE_PLACEHOLDER = "Reconnecting to desktop...";
export const READ_ONLY_REASON = "Read-only: this device can read sessions but not send messages.";
/** A wide screen with a mouse or trackpad: Enter sends there, and only there. */
export const DESKTOP_LAYOUT_QUERY = "(min-width: 640px) and (pointer: fine)";
/** A new Stop button keeps its press off for this long, so a quick second tap after Send cannot land on it. */
export const STOP_ARM_DELAY_MS = 500;
/** How long after a resolved cancel request Stop may be pressed again while the turn still runs. */
export const STOP_RETRY_MS = 4000;
export const UNKNOWN_ERROR_SENTENCE = "Something went wrong. Try again, or check the desktop.";
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
  /** A read-only device: lets the notice below the card ask the desktop for control (C-7). */
  requestControl?: RequestControl;
  /** The link is not ready: Send and Stop wait, writing does not. */
  offline?: boolean;
  /** The session's latest error: a compact row above the field (Details, Dismiss) until dismissed. */
  notice?: { text: string; dismiss: () => void } | null;
  /** Model controls and the like, between the add button and Send. */
  toolbar?: ReactNode;
  /** Above the field, inside the card (the workspace menu). */
  topRow?: ReactNode;
  /** A setup error under the pill (a model control failed); inside the padded wrapper, so the safe area stays below it. */
  configError?: string | null;
  autoFocus?: boolean;
  /** The draft to start from (a page that was unmounted and comes back), and a report of every change to it. */
  initialText?: string;
  onTextChange?: (text: string) => void;
  className?: string;
}

/** The desktop sometimes sends a bare code ("unknown") or nothing: say it in a plain sentence. */
export function plainErrorText(text: string): string {
  const trimmed = text.trim();
  return trimmed === "" || /^[a-z][a-z0-9_.-]*$/.test(trimmed) ? UNKNOWN_ERROR_SENTENCE : trimmed;
}

/** A turn the user stopped is not a failure; the desktop reports it only as text, so match its plain wording. */
// The desktop ends a stopped turn with the bare text "Stopped" (provider.rs `Err("Stopped".into())`); match only that.
function isCancelNotice(text: string): boolean {
  return /^Stopped\.?$/.test(text.trim());
}

/** True when focus came from the keyboard (`:focus-visible`); a pointer press on a button is not. */
function isKeyboardFocus(target: EventTarget): boolean {
  try {
    return target instanceof Element && target.matches(":focus-visible");
  } catch {
    return false;
  }
}

export function Composer({ onSubmit, busy = false, onStop, readOnly = false, requestControl, offline = false, notice, toolbar, topRow, configError, autoFocus = false, initialText = "", onTextChange, className }: ComposerProps) {
  const [text, setText] = useState(initialText);
  useEffect(() => {
    onTextChange?.(text);
  }, [text, onTextChange]);
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
  const shownNotice = notice && !isCancelNotice(notice.text) ? notice : null;
  const [detailsOpen, setDetailsOpen] = useState(false);
  const reducedMotion = useReducedMotion();
  const [focusWithin, setFocusWithin] = useState(false);
  // A press elsewhere on the page blurs the field, and collapsing the card then shrinks the composer and shifts the
  // transcript under the pointer, so the press lands on something else and its click is lost. The collapse waits
  // until the press ends (after the click has been dispatched).
  const pressed = useRef(false);
  const collapseTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const down = () => {
      pressed.current = true;
    };
    const up = () => {
      if (!pressed.current) return;
      pressed.current = false;
      window.clearTimeout(collapseTimer.current);
      collapseTimer.current = window.setTimeout(() => {
        if (!area.current?.closest("[data-shape]")?.contains(document.activeElement)) setFocusWithin(false);
      }, 0);
    };
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", up, true);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", up, true);
      window.clearTimeout(collapseTimer.current);
    };
  }, []);

  const hasDraft = text.trim().length > 0 || attachments.length > 0;
  const card = focusWithin || hasDraft || !!shownNotice || !!error;
  const fieldMin = card ? INPUT_MIN_PX : PILL_FIELD_MIN_PX;
  const canSend = hasDraft && !submitting && !readOnly && !offline;
  const showStop = busy && !!onStop;
  const showSend = !showStop || hasDraft;
  // Stop is armed STOP_ARM_DELAY_MS after it appears; until then a press does nothing.
  // It re-arms whenever it moves to another slot (alone, or left of Send once a draft exists), so a double tap on Send
  // never lands on an armed Stop. `armedSlot` names the slot it was armed in.
  const [armedSlot, setArmedSlot] = useState<boolean | null>(null);
  const stopArmed = showStop && armedSlot === hasDraft;
  useEffect(() => {
    if (!showStop) return;
    const timer = window.setTimeout(() => setArmedSlot(hasDraft), STOP_ARM_DELAY_MS);
    return () => {
      window.clearTimeout(timer);
      setArmedSlot(null);
    };
  }, [showStop, hasDraft]);
  // After the cancel request resolved and the turn still runs, Stop is pressable again (keeping its Stopping label).
  const [retryable, setRetryable] = useState(false);
  const retryTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(retryTimer.current), []);
  // Stopping lasts until the turn really ends (the desktop frame), not only until `session.cancel` resolves.
  // Adjusted during render when `busy` changes (React's pattern for state derived from a prop change).
  const [prevBusy, setPrevBusy] = useState(busy);
  if (busy !== prevBusy) {
    setPrevBusy(busy);
    if (!busy) {
      setStopping(false);
      setRetryable(false);
    }
  }
  useEffect(() => {
    if (!busy) window.clearTimeout(retryTimer.current);
  }, [busy]);

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

  // iOS may scroll the page to the focused field. The document no longer scrolls (the session column ends at
  // the viewport bottom), so this is only a harmless reset.
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
    // Phones, tablets and touch: close the on-screen keyboard right away, before anything is awaited. Doing it after the
    // send resolves could close it while the user already types the next message, and a send-guard dialog would hand the
    // focus back to the field when it closes (it restores to the element focused when it opened: with the field blurred
    // that is the page body). A refused or cancelled send keeps the draft but leaves the field unfocused, which is fine.
    // A desktop layout keeps the focus to type the next message.
    if (!desktopLayout) area.current?.blur();
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
    if (!onStop || !stopArmed || (stopping && !retryable) || offline || readOnly) return;
    setStopping(true);
    setRetryable(false);
    window.clearTimeout(retryTimer.current);
    setError(null);
    try {
      await onStop();
      retryTimer.current = window.setTimeout(() => setRetryable(true), STOP_RETRY_MS);
    } catch (reason) {
      setStopping(false);
      setError(reason instanceof Error && reason.message ? `Could not stop: ${reason.message}` : "Could not stop the session.");
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

  const noticeContent = shownNotice ? sessionErrorContent(plainErrorText(shownNotice.text)) : null;
  const noticeMessage = noticeContent ? noticeContent.message : "";
  const placeholder = offline ? OFFLINE_PLACEHOLDER : busy ? BUSY_PLACEHOLDER : COMPOSER_PLACEHOLDER;
  const describedBy = [readOnly ? reasonId : null, error ? errorId : null].filter(Boolean).join(" ") || undefined;

  return (
    // The space under the pill is applied once, here: 0.75rem above the on-screen keyboard (lifted by `inset`), and
    // otherwise the home-indicator safe area, never less than 0.75rem (a device without one keeps the small gap).
    <div data-testid="composer" className={cn("pt-1", inset ? "pb-3" : "pb-[max(env(safe-area-inset-bottom),0.75rem)]", className)} style={inset ? { marginBottom: inset } : undefined}>
      <Glass
        className={cn("flex", card ? "flex-col gap-1 px-3 pb-2 pt-3" : "min-h-[50px] flex-row items-center gap-1 px-1.5 py-[3px]")}
        data-shape={card ? "card" : "pill"}
        layout
        initial={false}
        animate={{ borderRadius: card ? CARD_RADIUS_PX : PILL_RADIUS_PX }}
        transition={reducedMotion ? { duration: 0 } : MORPH_TRANSITION}
        onFocus={(event) => {
          // The field opens the card, and so does keyboard focus (Tab into a clipped control). A pointer press on a
          // button (Stop, add) does not: morphing the layout then moves the button from under the pointer.
          if (event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement || isKeyboardFocus(event.target)) setFocusWithin(true);
        }}
        onMouseDown={(event) => {
          // A press on a composer button must not take focus from the field: in WebKit that blur (relatedTarget null)
          // collapses the card on pointerdown and moves the button. Keyboard activation is unaffected.
          if (event.target instanceof Element && event.target.closest("button") && !event.target.closest("[data-composer-extra]")) event.preventDefault();
        }}
        onBlur={(event) => {
          if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
          if (pressed.current) return;
          setFocusWithin(false);
        }}
        onDragOver={(event) => {
          if (event.dataTransfer.types.includes("Files")) event.preventDefault();
        }}
        onDrop={onDrop}
      >
        {topRow ? <div data-composer-extra className={card ? "flex min-w-0 items-center gap-2 px-1" : "size-0 overflow-hidden"}>{topRow}</div> : null}

        {attachments.length > 0 ? (
          <ul aria-label="Attachments" className="m-0 flex list-none gap-2 overflow-x-auto px-1 py-1">
            {attachments.map((attachment) => (
              <li key={attachment.id} className="shrink-0">
                <AttachmentChip attachment={attachment} onRemove={() => remove(attachment.id)} />
              </li>
            ))}
          </ul>
        ) : null}

        {shownNotice ? (
          <div role="alert" data-testid="composer-notice" className="flex min-w-0 items-center gap-1 rounded-row bg-fill pl-3 text-small">
            <span className="min-w-0 flex-1 py-1.5">
              <span className="block font-medium text-deletions">{noticeContent?.title}</span>
              <span className="block truncate text-muted-text">{noticeMessage}</span>
            </span>
            <button
              type="button"
              onClick={() => setDetailsOpen(true)}
              className="inline-flex min-h-11 shrink-0 items-center rounded-row px-2 text-content hover:bg-selection pointer-fine:min-h-8"
            >
              Details
            </button>
            <button
              type="button"
              onClick={shownNotice.dismiss}
              className="inline-flex min-h-11 shrink-0 items-center rounded-row px-2 text-muted-text hover:bg-selection pointer-fine:min-h-8"
            >
              Dismiss
            </button>
          </div>
        ) : null}

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

        <div className={card ? "flex min-w-0 items-center gap-2" : "contents"}>
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
            disabled={readOnly}
            onClick={() => picker.current?.click()}
            className={cn("inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection disabled:opacity-40 pointer-fine:size-9", !card && "order-1")}
          >
            <Icon name="add" size={18} />
          </button>
          <DictationButton onText={insertText} disabled={readOnly} className={cn("size-11 pointer-fine:size-9", !card && "order-2")} />
          <div data-composer-extra className={card ? "flex min-w-0 flex-1 items-center" : "order-4 size-0 overflow-hidden"}>{toolbar}</div>
          {showStop ? (
            <button
              type="button"
              aria-label={stopping ? undefined : "Stop"}
              title={stopping ? "Stopping..." : undefined}
              aria-busy={stopping}
              aria-disabled={!stopArmed || (stopping && !retryable) || undefined}
              disabled={offline || readOnly}
              onClick={() => void stop()}
              className={cn(
                "inline-flex h-11 shrink-0 items-center justify-center gap-2 rounded-pill bg-primary text-primary-foreground disabled:opacity-40 aria-disabled:opacity-60 pointer-fine:h-9",
                stopping ? "px-3 text-meta font-medium" : "w-11 pointer-fine:w-9",
                !card && "order-5",
              )}
            >
              {stopping ? <Icon name="spinner" size={16} className="animate-spin" /> : <span aria-hidden="true" className="block size-3 rounded-[3px] bg-current" />}
              {stopping ? <span>Stopping...</span> : null}
            </button>
          ) : null}
          {/* One polite live region, outside the button, so the state is announced without changing its name. */}
          <span role="status" className="sr-only">
            {showStop && stopping ? "Stopping..." : ""}
          </span>
          {showSend ? (
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
          ) : null}
        </div>
      </Glass>
      {configError ? (
        <p role="alert" data-testid="composer-config-error" className="px-4 pt-1 text-meta text-deletions">
          {configError}
        </p>
      ) : null}
      {readOnly ? (
        <p id={reasonId} className="px-4 pt-2 text-meta text-muted-text">
          {READ_ONLY_REASON}
        </p>
      ) : null}
      {readOnly && requestControl ? (
        <div className="flex items-center gap-2 px-2 pt-1 text-meta">
          {requestControl.state === "requested" ? <p role="status" className="px-2 text-muted-text">Requested, confirm on the desktop.</p> : null}
          <button
            type="button"
            disabled={requestControl.state === "pending" || requestControl.ready === false}
            title={requestControl.ready === false ? "Checking this device..." : undefined}
            onClick={requestControl.request}
            className="inline-flex min-h-11 items-center rounded-row px-2 font-medium text-content hover:bg-selection disabled:opacity-60 pointer-fine:min-h-8"
          >
            {requestControl.state === "pending" ? "Requesting..." : requestControl.state === "requested" ? "Ask again" : "Request control"}
            {requestControl.ready === false ? <span className="sr-only">. Checking this device...</span> : null}
          </button>
          {requestControl.state === "error" ? <p role="alert" className="text-deletions">Could not request control. Try again.</p> : null}
        </div>
      ) : null}
      {shownNotice ? <SessionErrorDialog message={plainErrorText(shownNotice.text)} open={detailsOpen} onOpenChange={setDetailsOpen} /> : null}
      {error ? (
        <p id={errorId} role="alert" className="px-4 pt-2 text-meta text-deletions">
          {error}
        </p>
      ) : null}
    </div>
  );
}
