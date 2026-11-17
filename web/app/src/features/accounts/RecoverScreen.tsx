import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Field } from "@/ui/Field";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { AddPasskeyStep } from "./AddPasskeyStep";
import { passkeyErrorMessage, recoverErrorMessage } from "./authCopy";
import { meQuery, ReauthCancelledError, recoverAccount, regenerateRecoveryCodes } from "./queries";
import { RecoveryCodes } from "./RecoveryCodes";
import { normalizeRecoveryCode } from "./recoveryCodesFile";
import { postSignInTarget } from "./redirect";

type Step = "form" | "passkey" | "regenerate" | "codes";

/** The server asks for new codes when this many or fewer are left. */
export const LOW_CODES_THRESHOLD = 2;

export function RecoverScreen() {
  const queryClient = useQueryClient();
  const router = useRouter();
  // A reload during a recovery session lands here again (the guard sends recoveryOnly sessions to
  // /recover), so the session itself decides the first step. A 401 simply means "signed out".
  const me = useQuery({ ...meQuery, retry: false });
  const [localStep, setLocalStep] = useState<Step | null>(null);
  const [handle, setHandle] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [codesLeft, setCodesLeft] = useState<number | null>(null);
  const [accountHandle, setAccountHandle] = useState("");
  const [newCodes, setNewCodes] = useState<string[]>([]);

  const step: Step = localStep ?? (me.data?.recoveryOnly ? "passkey" : "form");
  const resolvedHandle = accountHandle || me.data?.user.handle || handle.trim().toLowerCase();

  function finish() {
    setNewCodes([]);
    void router.navigate({ href: postSignInTarget(undefined) });
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const normalized = normalizeRecoveryCode(code);
    const cleanHandle = handle.trim().toLowerCase();
    setError(null);
    if (!cleanHandle || !normalized) {
      setError("Enter your handle and one recovery code.");
      return;
    }
    setBusy(true);
    try {
      const result = await recoverAccount(queryClient, { handle: cleanHandle, code: normalized });
      setAccountHandle(result.user.handle);
      setCodesLeft(result.codesLeft);
      setCode("");
      setLocalStep("passkey");
    } catch (caught) {
      setError(recoverErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  function afterPasskey() {
    if (codesLeft !== null && codesLeft <= LOW_CODES_THRESHOLD) setLocalStep("regenerate");
    else finish();
  }

  async function regenerate() {
    setBusy(true);
    setError(null);
    try {
      setNewCodes(await regenerateRecoveryCodes(queryClient));
      setLocalStep("codes");
    } catch (caught) {
      setError(caught instanceof ReauthCancelledError ? "The passkey prompt was closed. Try again when you are ready." : passkeyErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  if (!localStep && me.isPending) {
    return (
      <AuthLayout>
        <PageHeader title="Recover your account" />
        <DelayedSpinner label="Loading" />
      </AuthLayout>
    );
  }

  if (step === "passkey") {
    return (
      <AuthLayout>
        <PageHeader title="Add a new passkey" description="Your recovery code worked. Add a passkey on this device to finish signing in." />
        <AddPasskeyStep label="Add a passkey" onDone={afterPasskey} />
      </AuthLayout>
    );
  }

  if (step === "regenerate") {
    return (
      <AuthLayout>
        <PageHeader title="Get new recovery codes" description="Only a few of your recovery codes are left." />
        {error ? <Notice variant="error">{error}</Notice> : null}
        <div className="flex flex-col gap-2">
          <Button type="button" size="lg" className="w-full" disabled={busy} aria-busy={busy} onClick={() => void regenerate()}>
            Generate new codes
            {busy ? <DelayedSpinner label="Generating codes" /> : null}
          </Button>
          <Button type="button" variant="ghost" className="w-full" disabled={busy} onClick={finish}>
            Not now
          </Button>
        </div>
      </AuthLayout>
    );
  }

  if (step === "codes") {
    return (
      <AuthLayout>
        <PageHeader title="Save your new recovery codes" description="Older codes no longer work." />
        <RecoveryCodes handle={resolvedHandle} codes={newCodes} onContinue={finish} />
      </AuthLayout>
    );
  }

  return (
    <AuthLayout>
      <PageHeader title="Recover your account" description="Enter your handle and one of your recovery codes. You will add a new passkey next." />
      <form noValidate onSubmit={(event) => void submit(event)} className="flex flex-col gap-4">
        <Field
          label="Handle"
          name="handle"
          value={handle}
          onChange={(event) => setHandle(event.target.value)}
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <Field
          label="Recovery code"
          name="code"
          value={code}
          onChange={(event) => setCode(event.target.value)}
          hint="Spaces, dashes and lower case are fine."
          autoComplete="off"
          autoCapitalize="characters"
          autoCorrect="off"
          spellCheck={false}
          className="font-mono"
        />
        {error ? <Notice variant="error">{error}</Notice> : null}
        <Button type="submit" size="lg" className="w-full" disabled={busy} aria-busy={busy}>
          Recover account
          {busy ? <DelayedSpinner label="Checking your code" /> : null}
        </Button>
      </form>
      <p className="text-center text-small text-muted-text">
        Have your passkey?{" "}
        <Link to="/login" className="font-semibold text-brand-text hover:underline">
          Sign in
        </Link>
      </p>
    </AuthLayout>
  );
}
