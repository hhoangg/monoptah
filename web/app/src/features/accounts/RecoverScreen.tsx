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
import { recoverErrorMessage } from "./authCopy";
import { ApiError } from "./api";
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
  // The server lets a recovery-only session call GET /api/me and logout, and nothing else but
  // adding a passkey. A 403 `recovery_only` from `me` (an older server) means the same thing.
  const recoverySession = me.data?.recoveryOnly === true || (me.error instanceof ApiError && me.error.code === "recovery_only");
  const step: Step = localStep ?? (recoverySession ? "passkey" : "form");

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
      await recoverAccount(queryClient, { handle: cleanHandle, code: normalized });
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
  async function afterPasskey() {
    try {
      await signOut(queryClient);
    } catch {
      // The local state is cleared in signOut's finally block either way.
    }
    void router.navigate({ to: "/login" });
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
        <PageHeader title="Add a new passkey" description="Your recovery code worked. Add a passkey on this device, then sign in with it." />
        <AddPasskeyStep label="Add a passkey" onDone={() => void afterPasskey()} />
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
