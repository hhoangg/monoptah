import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { apiFetch } from "./api";
import { passkeyErrorMessage } from "./authCopy";
import { endpoints } from "./endpoints";
import { clearLowCodes } from "./lowCodes";
import { setRecoveryBatch } from "./pendingRecoveryCodes";
import { acknowledgeRecoveryCodes, ReauthCancelledError, withFreshAuth } from "./queries";
import { RecoveryCodes } from "./RecoveryCodes";

/**
 * Shown on /settings while `me.recoveryAckAt === null` (the guard sends every route there). The
 * user must create and save a fresh set of codes before the app opens up.
 *
 * It calls the endpoint directly instead of `regenerateRecoveryCodes`: that helper refreshes `me`
 * at once, which would swap this screen out and drop the codes before they are saved. `me` is
 * refreshed on Continue instead. Codes stay in this component's state only.
 */
export function ForcedRegenerate({ handle }: { handle: string }) {
  const queryClient = useQueryClient();
  const [codes, setCodes] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);

  async function generate() {
    setBusy(true);
    setError(null);
    setCancelled(false);
    try {
      const result = await withFreshAuth(() => apiFetch(endpoints.regenerateRecoveryCodes));
      setRecoveryBatch(result.batch);
      setCodes(result.recoveryCodes);
    } catch (caught) {
      if (caught instanceof ReauthCancelledError) setCancelled(true);
      else setError(passkeyErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  async function done() {
    // Cleared on Continue only: until then the codes stay on screen and nothing may unmount this component.
    // The server records "I saved these" first; on a failure the codes stay on screen so they are not lost.
    setError(null);
    try {
      await acknowledgeRecoveryCodes(queryClient);
    } catch (caught) {
      setError(passkeyErrorMessage(caught));
      return;
    }
    clearLowCodes();
    setCodes(null);
  }

  if (codes) {
    return (
      <AuthLayout>
        <PageHeader title="Save your new recovery codes" description="Older codes no longer work." />
        {error ? <Notice variant="error">{error}</Notice> : null}
        <RecoveryCodes handle={handle} codes={codes} onContinue={() => void done()} />
      </AuthLayout>
    );
  }

  return (
    <AuthLayout>
      <PageHeader title="Get new recovery codes" description="You signed in with a recovery code, or have not confirmed your codes yet, so only a few may be left. Create a new set and save it before you continue." />
      {error ? <Notice variant="error">{error}</Notice> : null}
      {cancelled ? <Notice variant="info">The passkey prompt was closed. Press the button when you are ready.</Notice> : null}
      <Button type="button" size="lg" className="w-full" disabled={busy} aria-busy={busy} onClick={() => void generate()}>
        Generate recovery codes
        {busy ? <DelayedSpinner label="Generating codes" /> : null}
      </Button>
    </AuthLayout>
  );
}
