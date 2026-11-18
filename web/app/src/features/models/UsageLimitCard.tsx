// Ported from mobile/src/features/sessions/UsageLimitCard.tsx (F-55). Eligibility, reset wording and
// scheduling belong to the desktop (`state` comes from the watch frame's `usageLimit`); this card only
// offers Resume and Resume at reset, with the offline hint and the inline error of the mobile card.
import { useRef, useState } from "react";
import type { UsageLimit } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { formatUsageLimitReset, usageLimitWaiting } from "@/features/sessions/usageLimit";

export interface UsageLimitCardProps {
  state: UsageLimit;
  /** The link to the desktop is ready. */
  connected?: boolean;
  /** A read-only device cannot continue the session. */
  readOnly?: boolean;
  /** Resume now. Rejects with an Error whose message is shown inline (see `useUsageLimitActions`). */
  onResume: () => Promise<void>;
  /** Arm (true) or cancel (false) the automatic resume at reset. */
  onResumeAtReset: (enabled: boolean) => Promise<void>;
  /** Clock for the reset line, for tests. */
  now?: number;
  className?: string;
}

export function UsageLimitCard({ state, connected = true, readOnly = false, onResume, onResumeAtReset, now, className }: UsageLimitCardProps) {
  const pending = useRef(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [openedAt] = useState(() => Date.now());

  const run = async (action: () => Promise<void>) => {
    if (pending.current) return;
    pending.current = true;
    setSending(true);
    setError(null);
    try {
      await action();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      pending.current = false;
      setSending(false);
    }
  };

  const blocked = sending || !connected || readOnly;
  const clock = now ?? openedAt;
  const resetsAt = state.resetsAt;
  return (
    <section role="status" aria-live="polite" className={cn("flex flex-col gap-1.5 rounded-card bg-fill p-3.5", className)} data-usage-limit-card>
      <h3 className="text-small font-semibold text-content">Usage limit reached</h3>
      <p className="text-small text-content">Your conversation is saved. {state.resetLabel}</p>
      {resetsAt != null && usageLimitWaiting({ resetsAt }, clock) ? <p className="text-meta text-muted-text">Resets {formatUsageLimitReset(resetsAt, clock)}</p> : null}
      <p className="text-meta text-muted-text">{state.limitation}</p>
      {!connected ? <p className="text-meta text-muted-text">Reconnect to desktop to change continuation settings.</p> : null}
      {readOnly ? <p className="text-meta text-muted-text">This device is read only and cannot continue the session.</p> : null}
      {error ? (
        <p role="alert" className="text-small text-content">
          {error} Try again using the controls below.
        </p>
      ) : null}
      <div className="flex flex-wrap gap-x-6 pt-1.5">
        <button
          type="button"
          disabled={blocked || !state.canResume}
          onClick={() => void run(onResume)}
          className="min-h-9 text-row font-semibold text-content disabled:opacity-40"
        >
          {sending ? "Updating..." : "Resume"}
        </button>
        <button
          type="button"
          role="switch"
          aria-checked={state.resumeAtReset}
          disabled={blocked || !state.canAutoResume}
          onClick={() => void run(() => onResumeAtReset(!state.resumeAtReset))}
          className="min-h-9 text-row font-semibold text-content disabled:opacity-40"
        >
          {state.resumeAtReset ? "Cancel automatic resume" : "Resume at reset"}
        </button>
      </div>
    </section>
  );
}
