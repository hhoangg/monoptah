import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { acknowledgeRecoveryCodes, regenerateRecoveryCodes } from "@/features/accounts/queries";
import { setRecoveryCodesShown } from "@/features/accounts/pendingRecoveryCodes";
import { RecoveryCodes } from "@/features/accounts/RecoveryCodes";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Notice } from "@/ui/Notice";
import { SETTINGS_ANCHOR, SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { isApiError } from "@/features/accounts/api";
import { CODES_REPLACED_MESSAGE } from "@/features/accounts/authCopy";
import { useAction } from "./useAction";

const describeError = (error: unknown): string | null => (isApiError(error, "conflict") ? CODES_REPLACED_MESSAGE : null);

/**
 * Recovery codes (N-04). Regenerating is a fresh-auth action and invalidates every older code. The
 * new codes live in this component's state only (never the query cache, a mutation or storage) and
 * are dropped when the user is done or leaves the page.
 */
export function RecoveryCodesSection({ handle }: { handle: string }) {
  const queryClient = useQueryClient();
  const action = useAction(describeError);
  const [confirming, setConfirming] = useState(false);
  const [codes, setCodes] = useState<string[] | null>(null);

  useEffect(() => () => setCodes(null), []);
  // Keep /settings on this page while the new codes are on screen: a `me` refetch would otherwise see the cleared ack
  // and swap in the forced regenerate screen, dropping the codes before they are saved.
  const showing = codes !== null;
  useEffect(() => {
    setRecoveryCodesShown(showing);
    return () => setRecoveryCodesShown(false);
  }, [showing]);

  async function generate() {
    const ok = await action.run(async () => setCodes(await regenerateRecoveryCodes(queryClient)));
    if (ok) setConfirming(false);
  }

  async function done() {
    // The Worker cleared the ack when it issued the codes: set it again now that the user saved them.
    const ok = await action.run(async () => {
      try {
        await acknowledgeRecoveryCodes(queryClient);
      } catch (caught) {
        // A newer batch exists (another tab regenerated): the codes on screen are dead, so drop them.
        if (isApiError(caught, "conflict")) setCodes(null);
        throw caught;
      }
    });
    if (ok) setCodes(null);
  }

  if (codes) {
    return (
      <SettingsCard id="recovery-codes" className={SETTINGS_ANCHOR} title="Recovery codes">
        <div className="flex flex-col gap-4 p-3.5">
          <p className="text-body font-semibold text-content">Your new recovery codes</p>
          <Notice variant="info">Your older codes no longer work.</Notice>
          {action.error ? <Notice variant="error">{action.error}</Notice> : null}
          <RecoveryCodes handle={handle} codes={codes} continueLabel="Done" onContinue={done} />
        </div>
      </SettingsCard>
    );
  }

  return (
    <SettingsCard id="recovery-codes" className={SETTINGS_ANCHOR} title="Recovery codes" footnote="Recovery codes get you back in if you lose every passkey. Keep them somewhere safe.">
      {action.error && !confirming ? (
        <div className="px-3.5 pt-3">
          <Notice variant="error">{action.error}</Notice>
        </div>
      ) : null}
      <SettingsRow icon="lock" title="Replace recovery codes" description="Creates 10 new one-time codes and turns off the old ones." />
      <div className="px-1.5 py-2">
        <Button
          type="button"
          variant="outline"
          className="w-full sm:w-auto"
          onClick={() => {
            action.reset();
            setConfirming(true);
          }}
        >
          Generate new codes
        </Button>
      </div>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title="Replace your recovery codes?"
        description="Your current codes stop working as soon as the new ones are created. You will be asked to confirm with a passkey."
        confirmLabel="Generate new codes"
        busy={action.busy}
        error={action.error}
        notice={action.notice}
        onConfirm={() => void generate()}
      />
    </SettingsCard>
  );
}
