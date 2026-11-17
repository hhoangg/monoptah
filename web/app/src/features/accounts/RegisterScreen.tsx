import { useQueryClient } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { RegisterOptionsBody } from "@monoptah/protocol";
import { useEffect, useState, type FormEvent } from "react";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Field } from "@/ui/Field";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { AddPasskeyStep } from "./AddPasskeyStep";
import { registerErrorMessage, UNSUPPORTED_ERROR } from "./authCopy";
import { RecoveryCodes } from "./RecoveryCodes";
import { postSignInTarget } from "./redirect";
import { registerAccount } from "./queries";
import { passkeysSupported } from "./webauthn";

type Step = "form" | "codes" | "passkey";

/** Reads `#invite=CODE` from the address bar. Pure: safe to run again if React retries a render. */
export function readInviteFromFragment(): string {
  const hash = window.location.hash;
  if (!hash) return "";
  return new URLSearchParams(hash.slice(1)).get("invite")?.trim() ?? "";
}

/** Removes the fragment from the address bar and history entry, keeping the router's history state. */
export function scrubInviteFragment(): void {
  if (!window.location.hash) return;
  window.history.replaceState(window.history.state, "", window.location.pathname + window.location.search);
}

interface FormErrors {
  invite?: string;
  handle?: string;
  displayName?: string;
}

function validate(values: { invite: string; handle: string; displayName: string }): FormErrors {
  const errors: FormErrors = {};
  if (!values.invite) errors.invite = "Enter the invite code from your link.";
  const parsed = RegisterOptionsBody.safeParse(values);
  if (!parsed.success) {
    for (const issue of parsed.error.issues) {
      const key = issue.path[0];
      if (key === "handle" && !errors.handle) errors.handle = "Use 3 to 32 lowercase letters, digits, - or _.";
      if (key === "displayName" && !errors.displayName) errors.displayName = "Enter a name of up to 64 characters.";
    }
  }
  return errors;
}

export function RegisterScreen() {
  const queryClient = useQueryClient();
  const router = useRouter();
  const [invite, setInvite] = useState(readInviteFromFragment);
  // After commit, so a discarded or retried render still finds the fragment (idempotent).
  useEffect(scrubInviteFragment, []);
  const [handle, setHandle] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const [busy, setBusy] = useState(false);
  const [step, setStep] = useState<Step>("form");
  // Recovery codes stay in this component's state only and vanish with it.
  const [codes, setCodes] = useState<string[]>([]);
  const [accountHandle, setAccountHandle] = useState("");
  const supported = passkeysSupported();

  function finish() {
    setCodes([]);
    void router.navigate({ href: postSignInTarget(undefined) });
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const values = { invite: invite.trim(), handle: handle.trim().toLowerCase(), displayName: displayName.trim() };
    const found = validate(values);
    setErrors(found);
    setFormError(null);
    setCancelled(false);
    if (Object.keys(found).length > 0) return;
    setBusy(true);
    try {
      const result = await registerAccount(queryClient, values);
      if (result.status === "cancelled") {
        setCancelled(true);
        return;
      }
      setAccountHandle(result.user.handle);
      setCodes(result.recoveryCodes);
      setStep("codes");
    } catch (caught) {
      setFormError(registerErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  if (step === "codes") {
    return (
      <AuthLayout>
        <PageHeader title="Save your recovery codes" description="They are the only way back in if you lose every passkey." />
        <RecoveryCodes handle={accountHandle} codes={codes} onContinue={() => setStep("passkey")} />
      </AuthLayout>
    );
  }

  if (step === "passkey") {
    return (
      <AuthLayout>
        <PageHeader title="Add a second passkey" description="A passkey on another device keeps you from being locked out." />
        <AddPasskeyStep skippable label="Add a second passkey" onDone={finish} onSkip={finish} />
      </AuthLayout>
    );
  }

  return (
    <AuthLayout>
      <PageHeader title="Create your account" description="Monoptah is invite only. You sign in with a passkey, no password." />
      <form noValidate onSubmit={(event) => void submit(event)} className="flex flex-col gap-4">
        <Field
          label="Invite code"
          name="invite"
          value={invite}
          onChange={(event) => setInvite(event.target.value)}
          error={errors.invite}
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          className="font-mono"
        />
        <Field
          label="Handle"
          name="handle"
          value={handle}
          onChange={(event) => setHandle(event.target.value)}
          error={errors.handle}
          hint="Your handle cannot be changed later."
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <Field
          label="Display name"
          name="displayName"
          value={displayName}
          onChange={(event) => setDisplayName(event.target.value)}
          error={errors.displayName}
          autoComplete="name"
        />
        {!supported ? <Notice variant="error">{UNSUPPORTED_ERROR}</Notice> : null}
        {formError ? <Notice variant="error">{formError}</Notice> : null}
        {cancelled ? <Notice variant="info">Passkey creation was cancelled. Press Create passkey to try again.</Notice> : null}
        <Button type="submit" size="lg" className="w-full" disabled={busy || !supported} aria-busy={busy}>
          <Icon name="fingerprint" size={20} />
          Create passkey
          {busy ? <DelayedSpinner label="Creating your account" /> : null}
        </Button>
      </form>
      <p className="text-center text-small text-muted-text">
        Already have an account?{" "}
        <Link to="/login" className="font-semibold text-brand-text hover:underline">
          Sign in
        </Link>
      </p>
    </AuthLayout>
  );
}
