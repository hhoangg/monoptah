// U-08: the pending tool approval above the composer, in the look of the question card (W-10).
//
// Security (security.md section 7, U-13): `label` and `detail` are untrusted provider text. They are rendered
// as plain React text nodes only: no markdown, no links, no HTML. The safe markdown renderer (W-07) has no
// plain-text mode, and a command preview needs none of its features, so React's text escaping is the smallest
// attack surface. There are exactly two actions, "Allow once" and "Deny"; nothing sends a mode, `runtimeMode`
// or a "full-access" value, and the only request this card sends is `session.answerApproval`.
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { Approval } from "@monoptah/protocol";
import { useApprovals, useLinkMutation } from "@/data";
import { useLinkState } from "@/link";
import { useCapability } from "@/version";
import { Button } from "@/ui/button";
import { Glass } from "@/ui/Glass";
import { Shield } from "@/ui/icons";
import { cn } from "@/lib/utils";
import { isApprovalCardEnabled } from "./flag";

export const APPROVAL_READ_ONLY_REASON = "Approvals need a device with control access.";
/** Longest label and detail shown; the desktop text is capped again here so a huge payload cannot flood the DOM. */
export const MAX_LABEL_CHARS = 300;
export const MAX_DETAIL_CHARS = 8000;

export type ApprovalDecision = "allow" | "deny";

function cap(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}...` : text;
}

export interface ApprovalCardViewProps {
  approvals: readonly Approval[];
  /** Sends the decision (`session.answerApproval`). A rejection shows an error and allows a retry. */
  onAnswer: (requestId: Approval["requestId"], decision: ApprovalDecision) => Promise<void>;
  /** When set the buttons are disabled and this text explains why. */
  disabledReason?: string;
  className?: string;
}

/** The card itself: one request at a time, two buttons, one decision per request id. */
export function ApprovalCardView({ approvals, onAnswer, disabledReason, className }: ApprovalCardViewProps) {
  const current = approvals[0];
  const [answered, setAnswered] = useState<Approval["requestId"] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Synchronous lock: two clicks in one tick must send once, before React re-renders the disabled state.
  const locked = useRef(false);
  const reasonId = useId();
  const titleId = useId();
  const readOnly = disabledReason != null;

  // The list of ids is the watch frame's verdict: once it changes, the answered mark and the error are stale.
  const signature = useMemo(() => approvals.map((approval) => String(approval.requestId)).join("\u0000"), [approvals]);
  const seenSignature = useRef(signature);
  useEffect(() => {
    if (seenSignature.current === signature) return;
    seenSignature.current = signature;
    locked.current = false;
    setAnswered(null);
    setError(null);
  }, [signature]);

  if (!current) return null;
  const done = answered === current.requestId;
  const busy = readOnly || done;

  const decide = async (decision: ApprovalDecision) => {
    if (readOnly || locked.current || answered === current.requestId) return;
    locked.current = true;
    setAnswered(current.requestId);
    setError(null);
    try {
      await onAnswer(current.requestId, decision);
      // Stay disabled: the result is shown from the next watch frame, never removed optimistically.
    } catch (reason) {
      locked.current = false;
      setAnswered(null);
      setError(reason instanceof Error && reason.message ? reason.message : "Answer not sent. Try again.");
    }
  };

  const label = cap(current.label.trim() || "Approval needed", MAX_LABEL_CHARS);
  const detail = cap(current.detail, MAX_DETAIL_CHARS);

  return (
    <section
      aria-labelledby={titleId}
      aria-describedby={readOnly ? reasonId : undefined}
      data-approval-card
      className={cn("px-3 pb-2", className)}
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

          {/* Announced when a new request arrives; keyed on the request so it is read again. */}
          <div role="status" aria-live="polite" key={String(current.requestId)} className="min-w-0">
            <p className="whitespace-pre-wrap break-words text-body font-medium text-content" data-approval-label>
              {label}
            </p>
            {detail ? (
              <pre
                tabIndex={0}
                aria-label="Request details"
                data-approval-detail
                className="mt-1.5 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-row border border-stroke px-3 py-2 font-mono text-small text-muted-text outline-offset-2"
              >
                {detail}
              </pre>
            ) : null}
          </div>

          {readOnly ? (
            <p id={reasonId} className="text-caption text-muted-text">
              {disabledReason}
            </p>
          ) : null}
          {done ? (
            <p role="status" className="text-caption text-muted-text">
              Answer sent. Waiting for the desktop.
            </p>
          ) : null}
          {error ? (
            <p role="alert" className="text-caption text-destructive">
              {error}
            </p>
          ) : null}

          <div className="mt-1 flex items-center justify-end gap-3">
            <Button type="button" variant="outline" size="pill" disabled={busy} onClick={() => void decide("deny")}>
              Deny
            </Button>
            <Button type="button" size="pill" disabled={busy} onClick={() => void decide("allow")}>
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
  const answer = useLinkMutation("session.answerApproval");
  if (!isApprovalCardEnabled() || !supported || approvals.length === 0) return null;
  return (
    <ApprovalCardView
      approvals={approvals}
      {...(permission !== "control" ? { disabledReason: APPROVAL_READ_ONLY_REASON } : {})}
      {...(className ? { className } : {})}
      onAnswer={async (requestId, decision) => {
        await answer.mutateAsync({ conversationId: sessionId, requestId, decision });
      }}
    />
  );
}
