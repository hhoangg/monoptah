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
import { markRecovered, recoverErrorMessage } from "./authCopy";
import { LOW_CODES_THRESHOLD, setLowCodes } from "./lowCodes";
import { ApiError, isApiError } from "./api";
import { meQuery, recoverAccount, signOut } from "./queries";
import { normalizeRecoveryCode } from "./recoveryCodesFile";

type Step = "form" | "passkey";

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
  const [fieldErrors, setFieldErrors] = useState<{ handle?: string; code?: string }>({});
  // The server lets a recovery-only session call GET /api/me and logout, and nothing else but
  // adding a passkey. A 403 `recovery_only` from `me` (an older server) means the same thing.
  const recoverySession = me.data?.recoveryOnly === true || (me.error instanceof ApiError && me.error.code === "recovery_only");
  const step: Step = localStep ?? (recoverySession ? "passkey" : "form");

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const normalized = normalizeRecoveryCode(code);
    const cleanHandle = handle.trim().toLowerCase();
    setError(null);
    const missing: { handle?: string; code?: string } = {};
    if (!cleanHandle) missing.handle = "Enter your handle.";
    if (!normalized) missing.code = "Enter one of your recovery codes.";
    setFieldErrors(missing);
    if (missing.handle || missing.code) {
      const first = missing.handle ? "handle" : "code";
      (event.currentTarget.elements.namedItem(first) as HTMLInputElement | null)?.focus();
      return;
    }
    setBusy(true);
    try {
      const result = await recoverAccount(queryClient, { handle: cleanHandle, code: normalized });
      if (result.codesLeft <= LOW_CODES_THRESHOLD) setLowCodes(result.user.handle);
      setCode("");
      setLocalStep("passkey");
    } catch (caught) {
      setError(recoverErrorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  /**
   * The server keeps a recovery session recovery-only even after it holds a passkey, so the user
   * signs out here and signs in with the new passkey. The cache is cleared even if logout fails.
   */
  async function leave(passkeyAdded: boolean) {
    setBusy(true);
    setError(null);
    try {
      await signOut(queryClient, { keepLowCodes: true });
    } catch (caught) {
      setBusy(false);
      // 401 means the session was already gone. Anything else keeps the user here: the recovery session
      // is still alive on the server and must be ended, so offer a retry.
      if (!isApiError(caught, "unauthorized")) {
        setError("Could not sign out. Check your connection and try again.");
        return;
      }
    }
    setBusy(false);
    if (passkeyAdded) markRecovered();
    void router.navigate({ to: "/login" });
  }

  if (!localStep && me.isPending) {
    return (
      <AuthLayout>
        <PageHeader title="Recover your account" />
        <DelayedSpinner label="Loading" className="text-muted-text" />
      </AuthLayout>
    );
  }

  if (step === "passkey") {
    return (
      <AuthLayout>
        <PageHeader title="Add a new passkey" description="Your recovery code worked. Add a passkey on this device, then sign in with it." />
        {error ? <Notice variant="error">{error}</Notice> : null}
        <AddPasskeyStep label="Add a passkey" onDone={() => void leave(true)} />
        {/* A recovery session lasts 30 minutes and can add a passkey without a re-auth: never leave it open on a shared browser. */}
        <Button type="button" variant="ghost" className="w-full" disabled={busy} onClick={() => void leave(false)}>
          Cancel and sign out
        </Button>
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
          onChange={(event) => {
            setHandle(event.target.value);
            setFieldErrors((current) => ({ ...current, handle: undefined }));
            setError(null);
          }}
          error={fieldErrors.handle}
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <Field
          label="Recovery code"
          name="code"
          value={code}
          onChange={(event) => {
            setCode(event.target.value);
            setFieldErrors((current) => ({ ...current, code: undefined }));
            setError(null);
          }}
          error={fieldErrors.code}
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
