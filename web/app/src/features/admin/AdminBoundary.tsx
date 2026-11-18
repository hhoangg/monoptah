import { useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { reauth, ReauthCancelledError } from "@/features/accounts/queries";
import { useAction } from "@/features/settings/useAction";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ADMIN_KEY, isForbidden, needsFreshSession } from "./adminApi";
import { NOT_AVAILABLE } from "./copy";

/** What a section needs from a query result (works for plain and infinite queries). */
export interface BoundaryQuery<T> {
  data: T | undefined;
  error: unknown;
  isError: boolean;
  refetch: () => unknown;
}

/**
 * The "Confirm it's you" step. Every admin route needs a fresh passkey check (5 minutes), so a read
 * that answers `fresh_required` shows this instead of a prompt nobody asked for. Confirming runs the
 * re-auth ceremony once and refreshes every admin query.
 */
export function ConfirmItsYou() {
  const queryClient = useQueryClient();
  const action = useAction();

  async function onConfirm() {
    await action.run(async () => {
      const result = await reauth();
      if (result.status === "cancelled") throw new ReauthCancelledError();
      await queryClient.invalidateQueries({ queryKey: ADMIN_KEY });
    });
  }

  return (
    <section aria-labelledby="confirm-its-you" className="flex flex-col items-start gap-3 rounded-card bg-surface p-5">
      <span className="inline-flex size-9 items-center justify-center rounded-pill bg-fill text-muted-text">
        <Icon name="lock" size={18} />
      </span>
      <h2 id="confirm-its-you" className="text-body font-semibold text-content">
        Confirm it&apos;s you
      </h2>
      <p className="text-small text-muted-text">
        Admin pages ask for your passkey again every 5 minutes. Nothing is shown until you confirm.
      </p>
      {action.error ? <Notice variant="error" className="w-full">{action.error}</Notice> : null}
      {action.notice ? <Notice variant="info" className="w-full">{action.notice}</Notice> : null}
      <Button type="button" disabled={action.busy} aria-busy={action.busy} onClick={() => void onConfirm()}>
        <Icon name="fingerprint" size={18} />
        Confirm with passkey
      </Button>
    </section>
  );
}

/** Loading after 250 ms, "Confirm it's you" for a stale session, "not available" for a 403, else the content. */
export function AdminBoundary<T>({ query, label, children }: { query: BoundaryQuery<T>; label: string; children: (data: T) => ReactNode }) {
  if (query.data !== undefined) return <>{children(query.data)}</>;
  if (query.isError) {
    if (needsFreshSession(query.error)) return <ConfirmItsYou />;
    if (isForbidden(query.error)) return <Notice variant="error" title="Not available">{NOT_AVAILABLE}</Notice>;
    return (
      <div className="flex flex-col items-start gap-3 py-2">
        <Notice variant="error" className="w-full">{`Could not load ${label}.`}</Notice>
        <Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>
          Try again
        </Button>
      </div>
    );
  }
  return (
    <div className="flex min-h-13 items-center justify-center py-2">
      <DelayedSpinner label={`Loading ${label}`} />
    </div>
  );
}
