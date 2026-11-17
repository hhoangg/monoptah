import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { signInErrorMessage, UNSUPPORTED_ERROR } from "./authCopy";
import { postSignInTarget } from "./redirect";
import { signIn } from "./queries";
import { passkeysSupported } from "./webauthn";

export function LoginScreen({ redirect }: { redirect?: string }) {
  const queryClient = useQueryClient();
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const supported = passkeysSupported();

  async function submit() {
    setBusy(true);
    setError(null);
    setCancelled(false);
    try {
      const result = await signIn(queryClient);
      if (result.status === "cancelled") {
        setCancelled(true);
        return;
      }
      await router.navigate({ href: postSignInTarget(redirect) });
    } catch (caught) {
      setError(signInErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  return (
    <AuthLayout>
      <PageHeader size="display" title="Sign in to Monoptah" description="Use the passkey saved on this device." />
      {!supported ? <Notice variant="error">{UNSUPPORTED_ERROR}</Notice> : null}
      {error ? <Notice variant="error">{error}</Notice> : null}
      {cancelled ? <Notice variant="info">Sign-in was cancelled. Try again when you are ready.</Notice> : null}
      <div className="flex flex-col gap-2">
        <Button type="button" size="lg" className="w-full" disabled={busy || !supported} aria-busy={busy} onClick={() => void submit()}>
          <Icon name="fingerprint" size={20} />
          Sign in with passkey
          {busy ? <DelayedSpinner label="Waiting for your passkey" /> : null}
        </Button>
        <p className="text-center text-meta text-muted-text">If your browser offers it, you can also use a passkey from your phone.</p>
      </div>
      <SettingsCard>
        <SettingsRow icon="key" title="Lost your passkey?" description="Use a recovery code" to="/recover" />
        <SettingsRow icon="download" title="Install the app" description="Add Monoptah to your home screen" to="/install" />
      </SettingsCard>
    </AuthLayout>
  );
}
