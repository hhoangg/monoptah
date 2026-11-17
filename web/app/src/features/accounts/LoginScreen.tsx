import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { isApiError } from "./api";
import { signInErrorMessage, UNSUPPORTED_ERROR } from "./authCopy";
import { postSignInTarget } from "./redirect";
import { meQuery, signIn, signOut } from "./queries";
import { passkeysSupported } from "./webauthn";

export function LoginScreen({ redirect }: { redirect?: string }) {
  const queryClient = useQueryClient();
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const supported = passkeysSupported();
  // /login never redirects on its own: a visitor with a session (possibly a stale one, when the
  // desktop link asked for a sign-in) chooses to continue or to sign in with another passkey.
  const me = useQuery({ ...meQuery, retry: false, refetchOnMount: "always" });
  const [differentPasskey, setDifferentPasskey] = useState(false);
  const account = me.data && !me.data.recoveryOnly && !differentPasskey ? me.data.user : null;

  async function switchPasskey() {
    setBusy(true);
    setError(null);
    try {
      await signOut(queryClient);
      setDifferentPasskey(true);
    } catch (caught) {
      // 401 means the session was already gone; anything else keeps the panel so the user can retry.
      if (isApiError(caught, "unauthorized")) setDifferentPasskey(true);
      else setError(signInErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

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

  if (account) {
    return (
      <AuthLayout>
        <PageHeader size="display" title="Sign in to Monoptah" description={`Signed in as ${account.displayName} (@${account.handle})`} />
        {error ? <Notice variant="error">{error}</Notice> : null}
        <div className="flex flex-col gap-2">
          <Button type="button" size="lg" className="w-full" disabled={busy} onClick={() => void router.navigate({ href: postSignInTarget(redirect) })}>
            Continue
          </Button>
          <Button type="button" variant="outline" size="lg" className="w-full" disabled={busy} aria-busy={busy} onClick={() => void switchPasskey()}>
            Sign in with a different passkey
            {busy ? <DelayedSpinner label="Signing out" /> : null}
          </Button>
        </div>
      </AuthLayout>
    );
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
