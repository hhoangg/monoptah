import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { passkeyErrorMessage, UNSUPPORTED_ERROR } from "./authCopy";
import { ReauthCancelledError, addPasskey } from "./queries";
import { passkeysSupported } from "./webauthn";

export interface AddPasskeyStepProps {
  /** Shows a "Skip for now" button; the forced recovery step leaves it out. */
  skippable?: boolean;
  /** Called after the passkey is saved. */
  onDone: () => void;
  onSkip?: () => void;
  label?: string;
}

/** Body of the "add a passkey" screens (second passkey after register, forced step after recovery). */
export function AddPasskeyStep({ skippable = false, onDone, onSkip, label = "Add a passkey" }: AddPasskeyStepProps) {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const supported = passkeysSupported();

  async function add() {
    setBusy(true);
    setError(null);
    setCancelled(false);
    try {
      const result = await addPasskey(queryClient);
      if (result.status === "cancelled") setCancelled(true);
      else onDone();
    } catch (caught) {
      if (caught instanceof ReauthCancelledError) setCancelled(true);
      else setError(passkeyErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      {!supported ? <Notice variant="error">{UNSUPPORTED_ERROR}</Notice> : null}
      {error ? <Notice variant="error">{error}</Notice> : null}
      {cancelled ? <Notice variant="info">The passkey prompt was closed. Press the button when you are ready.</Notice> : null}
      <Button type="button" size="lg" className="w-full" disabled={busy || !supported} aria-busy={busy} onClick={() => void add()}>
        <Icon name="fingerprint" size={20} />
        {label}
        {busy ? <DelayedSpinner label="Waiting for your passkey" /> : null}
      </Button>
      {skippable ? (
        <Button type="button" variant="ghost" className="w-full" disabled={busy} onClick={onSkip}>
          Skip for now
        </Button>
      ) : null}
    </div>
  );
}
