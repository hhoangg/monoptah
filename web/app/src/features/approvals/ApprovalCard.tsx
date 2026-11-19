// U-08: the pending tool approval above the composer, in the look of the question card (W-10).
//
// Security (security.md section 7, U-13): `label` and `detail` are untrusted provider text. They are rendered
// as plain React text nodes only: no markdown, no links, no HTML. The safe markdown renderer (W-07) has no
// plain-text mode, and a command preview needs none of its features, so React's text escaping is the smallest
// attack surface. Hidden characters are shown as visible markers (text.ts). There are exactly two actions,
// "Allow once" and "Deny"; nothing sends a mode, `runtimeMode` or a "full-access" value, and the only request
// this card sends is `session.answerApproval`.
//
// One decision per request: an id stays locked from the click until it leaves the list (a later frame), whatever
// else arrives meanwhile. A request that just appeared is armed only after ARM_DELAY_MS, and a press that began
// on another request is ignored, so a tap meant for one request never decides the next.
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import type { Approval } from "@monoptah/protocol";
import { useApprovals, useApprovalsSeq, useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { useCapability } from "@/version";
import { Button } from "@/ui/button";
import { Glass } from "@/ui/Glass";
import { Shield } from "@/ui/icons";
import { cn } from "@/lib/utils";
import { isApprovalCardEnabled } from "./flag";
import { showDetail, showLabel, type TextSegment } from "./text";

export const APPROVAL_READ_ONLY_REASON = "Approvals need a device with control access.";
export const APPROVAL_CONNECTING_REASON = "Connecting. Approvals unlock once the link is ready.";
export const APPROVAL_LINK_NOT_READY_REASON = "The link to your desktop is not ready. Approvals unlock once it is back.";
export const APPROVAL_UNCONFIRMED = "Could not confirm the answer; it may have reached the desktop. Wait for the desktop to update.";
export const APPROVAL_NEEDS_DESKTOP = "Answer this one on the desktop. It cannot be allowed once from here. You can still deny it.";
export const APPROVAL_NOT_PENDING = "This request is no longer waiting. The card updates when the desktop sends the next list.";
export const APPROVAL_STALE_REASON = "The link was interrupted. Approvals unlock once the desktop confirms this request again.";
export const APPROVAL_CUT_NOTICE =
  "Only the start of this request reached your browser. Review the full request on the desktop. Allow once is off because the whole request cannot be shown here.";
export const APPROVAL_TRUNCATED_NOTICE = "Review the full request on the desktop. Allow once is off because the whole request cannot be shown here.";
/** Longest label and detail shown; the desktop text is capped again here so a huge payload cannot flood the DOM. */
export const MAX_LABEL_CHARS = 300;
export const MAX_DETAIL_CHARS = 8000;
/** A new request keeps both buttons off for this long, so a tap aimed at the previous one cannot decide it. */
export const ARM_DELAY_MS = 700;
/** Detail with more lines than fit the box shows a scroll cue. */
export const SCROLL_CUE_LINES = 10;
const MAX_ERROR_CHARS = 120;

export type ApprovalDecision = "allow" | "deny";

// The label is a short title: isolate keeps it from reordering what is around it. The detail is a command
// preview, so it is shown in code point order: `bidi-override` with `direction: ltr` stops real Hebrew or
// Arabic letters from reordering the digits and punctuation next to them (security re-review N-2).
const isolate = { unicodeBidi: "isolate" } as const;
const codeOrder = { unicodeBidi: "bidi-override", direction: "ltr" } as const;

function Segments({ segments }: { segments: TextSegment[] }): ReactNode {
  return segments.map((segment, index) =>
    segment.marker ? (
      <span key={index} data-approval-marker className="rounded-sm bg-selection px-0.5 text-caution">
        {segment.text}
      </span>
    ) : (
      segment.text
    ),
  );
}

/** The desktop `code` a rejected answer carries (`LinkRequestError.code`), when it has one. */
function failureCode(reason: unknown): string | undefined {
  const code = (reason as { code?: unknown } | null)?.code;
  return typeof code === "string" ? code : undefined;
}

function errorDetail(reason: unknown): string {
  if (!(reason instanceof Error)) return "";
  const text = reason.message.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, " ").replace(/\s+/g, " ").trim();
  return text.length > MAX_ERROR_CHARS ? `${text.slice(0, MAX_ERROR_CHARS)}...` : text;
}

export interface ApprovalCardViewProps {
  approvals: readonly Approval[];
  /** Sends the decision (`session.answerApproval`). A rejection keeps the request locked (the answer may have landed). */
  onAnswer: (requestId: string, decision: ApprovalDecision) => Promise<void>;
  /** When set the buttons are disabled and this text explains why. */
  disabledReason?: string;
  className?: string;
}

/** The card itself: one request at a time, two buttons, one decision per request id. */
export function ApprovalCardView({ approvals, onAnswer, disabledReason, className }: ApprovalCardViewProps) {
  const current = approvals[0];
  const currentId = current ? String(current.requestId) : null;
  // Arming and presses follow the content too: a frame that changes the text of the same id disarms the buttons.
  const armKey = current ? `${currentId}\u0000${current.label}\u0000${current.detail ?? ""}` : null;
  const [answered, setAnswered] = useState<ReadonlySet<string>>(() => new Set());
  const [errors, setErrors] = useState<ReadonlyMap<string, string>>(() => new Map());
  // Ids the desktop refused to allow from here (`needs_desktop`): Deny stays available, Allow never is.
  const [desktopOnly, setDesktopOnly] = useState<ReadonlySet<string>>(() => new Set());
  const [armedFor, setArmedFor] = useState<string | null>(null);
  // Synchronous lock: two clicks in one tick must send once, before React re-renders the disabled state.
  const locked = useRef(new Set<string>());
  // The request a press (pointer down or key down) began on; a click only counts for that same request.
  const pressedOn = useRef<string | null>(null);
  // True while keyboard focus is inside the card, so a swap that disables the focused button cannot drop it to the page.
  const focusInside = useRef(false);
  const section = useRef<HTMLElement>(null);
  const reasonId = useId();
  const titleId = useId();
  const readOnly = disabledReason != null;

  // The ids in the list are the watch frame's verdict: an id that left it is settled, and only then forgotten.
  const ids = useMemo(() => approvals.map((approval) => String(approval.requestId)), [approvals]);
  const signature = ids.join("\u0000");
  const [seenSignature, setSeenSignature] = useState(signature);
  if (seenSignature !== signature) {
    // Adjusted during render (not in an effect) so no frame is ever drawn with a stale lock.
    const live = new Set(ids);
    setSeenSignature(signature);
    setAnswered((previous) => new Set([...previous].filter((id) => live.has(id))));
    setErrors((previous) => new Map([...previous].filter(([id]) => live.has(id))));
    setDesktopOnly((previous) => new Set([...previous].filter((id) => live.has(id))));
  }
  useEffect(() => {
    const live = new Set(signature === "" ? [] : signature.split("\u0000"));
    for (const id of [...locked.current]) if (!live.has(id)) locked.current.delete(id);
  }, [signature]);

  // Arm the buttons only a moment after a request (or its text) appears.
  useEffect(() => {
    if (armKey === null) return;
    const timer = setTimeout(() => setArmedFor(armKey), ARM_DELAY_MS);
    return () => {
      clearTimeout(timer);
      setArmedFor(null);
    };
  }, [armKey]);

  // Keep focus on the card when it was inside it: the focused button may turn disabled on a swap, and a browser
  // would then move focus to the page body. Layout effect: runs before the browser's focus fixup.
  useLayoutEffect(() => {
    const node = section.current;
    if (!node || !focusInside.current) return;
    const active = document.activeElement;
    if (active === null || active === document.body || node.contains(active)) node.focus();
  }, [armKey, answered]);
  useEffect(() => {
    const leave = (event: Event) => {
      if (!section.current?.contains(event.target as Node)) focusInside.current = false;
    };
    document.addEventListener("pointerdown", leave, true);
    return () => document.removeEventListener("pointerdown", leave, true);
  }, []);

  const label = useMemo(() => showLabel(current?.label ?? "", MAX_LABEL_CHARS), [current?.label]);
  const detail = useMemo(() => showDetail(current?.detail ?? "", MAX_DETAIL_CHARS), [current?.detail]);

  if (!current || currentId === null || armKey === null) return null;
  const done = answered.has(currentId);
  const armed = armedFor === armKey;
  const error = errors.get(currentId);
  const busy = readOnly || done || !armed;
  const labelText = label.map((segment) => segment.text).join("");
  // The desktop cut the text (its own cap) or this card did: the whole request is not on screen, so only Deny works.
  const cutByDesktop = current.truncated === true;
  const needsDesktop = desktopOnly.has(currentId);
  const cut = cutByDesktop || detail.truncated || needsDesktop;

  const decide = async (decision: ApprovalDecision) => {
    const pressed = pressedOn.current;
    pressedOn.current = null;
    if (pressed !== null && pressed !== armKey) return;
    if (readOnly || !armed || locked.current.has(currentId) || answered.has(currentId)) return;
    if (decision === "allow" && cut) return;
    const id = currentId;
    locked.current.add(id);
    setAnswered((previous) => new Set(previous).add(id));
    setErrors((previous) => {
      const next = new Map(previous);
      next.delete(id);
      return next;
    });
    // The pressed button turns disabled; keep keyboard focus on the card rather than lose it to the page.
    focusInside.current = true;
    section.current?.focus();
    try {
      await onAnswer(current.requestId, decision);
      // Stay locked: the result is shown from the next watch frame, never removed optimistically.
    } catch (reason) {
      const code = failureCode(reason);
      if (code === "not_pending") {
        // Nothing was decided: the request is already gone. Stay disarmed until the next frame drops it.
        setErrors((previous) => new Map(previous).set(id, APPROVAL_NOT_PENDING));
        return;
      }
      if (code === "needs_desktop") {
        // Nothing was decided and the desktop accepts a deny: unlock the card, with Allow off for this request.
        locked.current.delete(id);
        setDesktopOnly((previous) => new Set(previous).add(id));
        setAnswered((previous) => {
          const next = new Set(previous);
          next.delete(id);
          return next;
        });
        setErrors((previous) => new Map(previous).set(id, APPROVAL_NEEDS_DESKTOP));
        return;
      }
      // Stay locked too. The answer may have reached the desktop, and a second decision for the same request
      // (a different one, even) must not be sent before the desktop's frame says what happened.
      const extra = errorDetail(reason);
      setErrors((previous) => new Map(previous).set(id, extra ? `${APPROVAL_UNCONFIRMED} (${extra})` : APPROVAL_UNCONFIRMED));
    }
  };

  const press = (event: PointerEvent<HTMLButtonElement> | KeyboardEvent<HTMLButtonElement>) => {
    if ("key" in event && event.key !== "Enter" && event.key !== " ") return;
    pressedOn.current = armKey;
  };
  const overflows = detail.lines > SCROLL_CUE_LINES;
  const announcement = done ? "Answer sent. Waiting for the desktop." : `New approval request: ${labelText}`;

  return (
    <section
      ref={section}
      tabIndex={-1}
      aria-labelledby={titleId}
      aria-describedby={readOnly ? reasonId : undefined}
      data-approval-card
      onFocus={() => (focusInside.current = true)}
      className={cn("pb-2 focus-visible:outline-2 focus-visible:outline-brand", className)}
    >
      <Glass className="rounded-card">
        <div className="flex flex-col gap-2 px-4 py-3">
          <div className="flex items-center gap-1.5">
            <Shield size={14} className="shrink-0 text-muted-text" aria-hidden="true" />
            <span id={titleId} className="min-w-0 flex-1 truncate text-small text-muted-text">
              Needs your approval
            </span>
            {approvals.length > 1 ? (
              <span className="shrink-0 text-caption text-muted-text">1 of {approvals.length}</span>
            ) : null}
          </div>

          {/* One live region for the card's life: a short announcement, never the whole detail. */}
          <div role="status" aria-live="polite" className="sr-only">
            {announcement}
          </div>

          <div className="min-w-0">
            <p dir="ltr" style={isolate} className="whitespace-pre-wrap break-words text-body font-medium text-content" data-approval-label>
              <Segments segments={label} />
            </p>
            {detail.segments.length > 0 ? (
              <>
                <pre
                  dir="ltr"
                  style={codeOrder}
                  tabIndex={0}
                  aria-label="Request details"
                  data-approval-detail
                  className="mt-1.5 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-row border border-stroke px-3 py-2 font-mono text-small text-muted-text outline-offset-2"
                >
                  <Segments segments={detail.segments} />
                </pre>
                {overflows ? (
                  <p className="mt-1 text-caption text-muted-text" data-approval-meta>
                    {detail.lines} lines. Scroll to see all.
                  </p>
                ) : null}
              </>
            ) : null}
          </div>

          {cutByDesktop ? (
            <p role="alert" className="text-caption text-caution" data-approval-cut>
              {APPROVAL_CUT_NOTICE}
            </p>
          ) : detail.truncated ? (
            <p role="alert" className="text-caption text-caution" data-approval-truncated>
              Showing the first {MAX_DETAIL_CHARS.toLocaleString("en-US")} of {detail.total.toLocaleString("en-US")} characters. {APPROVAL_TRUNCATED_NOTICE}
            </p>
          ) : null}
          {readOnly ? (
            <p id={reasonId} className="text-caption text-muted-text">
              {disabledReason}
            </p>
          ) : !armed && !done ? (
            <p className="text-caption text-muted-text">New request. Buttons unlock in a moment.</p>
          ) : null}
          {done && !error ? <p className="text-caption text-muted-text">Answer sent. Waiting for the desktop.</p> : null}
          {error ? (
            <p role="alert" className="text-caption text-destructive">
              {error}
            </p>
          ) : null}

          <div className="mt-1 flex items-center justify-end gap-3">
            <Button type="button" onPointerDown={press} onKeyDown={press} onClick={() => void decide("deny")} variant="outline" size="pill" disabled={busy}>
              Deny
            </Button>
            <Button type="button" onPointerDown={press} onKeyDown={press} onClick={() => void decide("allow")} size="pill" disabled={busy || cut}>
              Allow once
            </Button>
          </div>
        </div>
      </Glass>
    </section>
  );
}

/** Feeds the card from `useApprovals`. Renders nothing while the flag is off or the desktop lacks `approvals`. */
export function ApprovalCard({ sessionId, className }: { sessionId: string; className?: string }) {
  const supported = useCapability("approvals");
  const approvals = useApprovals(sessionId);
  const permission = useLinkState((state) => state.permission);
  const status = useLinkState((state) => state.status);
  const seq = useApprovalsSeq(sessionId);
  const answer = useLinkMutation("session.answerApproval");
  // Fresh means the desktop sent the list after the link last left `ready`: a list that outlived a reconnect (or a
  // failed reopen) is only a memory, and request ids are reused, so it never decides anything. Adjusted during
  // render so no frame is drawn armed first.
  const [floor, setFloor] = useState(0);
  const [lastStatus, setLastStatus] = useState(status);
  if (lastStatus !== status) {
    setLastStatus(status);
    if (lastStatus === "ready") setFloor(seq + 1);
  }
  if (!isApprovalCardEnabled() || !supported || approvals.length === 0) return null;
  // Fail closed: nothing but an explicit `control` enables the buttons. `null` is a link that is still connecting.
  // The list is a frame from the last good link: while the link is not ready it may be stale, so answering waits.
  const reason =
    permission !== "control" ? (permission === null ? APPROVAL_CONNECTING_REASON : APPROVAL_READ_ONLY_REASON) : status !== "ready" ? APPROVAL_LINK_NOT_READY_REASON : seq < floor ? APPROVAL_STALE_REASON : undefined;
  return (
    <ApprovalCardView
      key={sessionId}
      approvals={approvals}
      {...(reason ? { disabledReason: reason } : {})}
      {...(className ? { className } : {})}
      onAnswer={async (requestId, decision) => {
        await answer.mutateAsync({ conversationId: sessionId, requestId, decision });
      }}
    />
  );
}
