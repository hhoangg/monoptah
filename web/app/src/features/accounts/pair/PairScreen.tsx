// /pair/:code (W-03, architecture.md 6.4): the signed-in user approves or denies a desktop pairing.
//
// What protects the user here is the comparison, so the screen is built around it:
//  - `#fp` (full fingerprint from the desktop's link or QR) is compared with the server's value. A
//    mismatch blocks: there is no Approve control at all, only Deny and Cancel (Cancel denies, then leaves).
//  - A link without `#fp` (a code typed or pasted bare) still works, but shows a warning and asks the
//    user to confirm that the code and fingerprint match the desktop screen. Decision: approval stays
//    possible because the QR and the links the desktop shows always carry `#fp`, and refusing every bare
//    code would make a typed code useless; the checkbox makes the comparison an explicit act.
//  - Approve needs a fresh passkey (`withFreshAuth`); Deny needs only the session.
//  - Machine name and OS are untrusted text: shown as text, stripped of hidden characters, truncated.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useRouterState } from "@tanstack/react-router";
import { useEffect, useState, type ReactNode } from "react";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button } from "@/ui/button";
import { Checkbox } from "@/ui/Checkbox";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { Radio } from "@/ui/Radio";
import { apiFetch, isApiError } from "../api";
import { GENERIC_ERROR, NETWORK_ERROR } from "../authCopy";
import { endpoints } from "../endpoints";
import { desktopsQuery, ReauthCancelledError, withFreshAuth } from "../queries";
import { checkFingerprint, fingerprintLines, type FingerprintCheck } from "./fingerprint";
import { safeLabel } from "./untrusted";

/** The F-03 copy, shown for every code that cannot be used: expired, unknown, used or malformed. */
export const PAIR_EXPIRED_MESSAGE = "This code expired. Choose Pair on your desktop to make a new one.";
export const PAIR_QUOTA_MESSAGE = "You have reached the desktop limit. Choose a desktop to replace, or remove one first.";
export const PAIR_DESKTOP_GONE_MESSAGE = "That desktop is no longer on your account. Choose another option.";

const NEW_DESKTOP = "new";
/** setTimeout cannot take more than 2^31 - 1 ms. */
const MAX_TIMER_MS = 2_147_483_647;

type Outcome = { kind: "approved"; name: string; replaced: boolean } | { kind: "denied" } | { kind: "gone" };

/** 404 and 400 are every "this code is not usable" answer of the Worker (unknown, expired, used, malformed). */
function isGone(error: unknown): boolean {
  return isApiError(error) && (error.status === 404 || error.status === 400);
}

function pairErrorMessage(error: unknown): string {
  if (isApiError(error, "network")) return NETWORK_ERROR;
  if (isApiError(error, "quota_exceeded")) return PAIR_QUOTA_MESSAGE;
  if (isApiError(error, "rate_limited")) return "Too many attempts. Wait a minute and try again.";
  return GENERIC_ERROR;
}

function BackToDesktops({ children = "Go to my desktops", variant = "default" }: { children?: ReactNode; variant?: "default" | "outline" }) {
  return (
    <Button asChild size="lg" variant={variant} className="w-full">
      <Link to="/desktops">{children}</Link>
    </Button>
  );
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-meta text-muted-text">{label}</dt>
      <dd className="min-w-0 text-body wrap-anywhere text-content">{children}</dd>
    </div>
  );
}

export function PairScreen({ code }: { code: string }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const hash = useRouterState({ select: (state) => state.location.hash });
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [busy, setBusy] = useState<"approve" | "deny" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancelled, setCancelled] = useState(false);
  const [choice, setChoice] = useState(NEW_DESKTOP);
  const [comparedByEye, setComparedByEye] = useState(false);

  // One read per visit: after approve or deny the request is no longer pending, so a refetch would 404.
  const info = useQuery({
    queryKey: ["pair", code],
    queryFn: ({ signal }) => apiFetch(endpoints.pairInfo, { params: { userCode: code }, signal }),
    retry: false,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: 0,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const desktops = useQuery({ ...desktopsQuery, enabled: info.isSuccess, retry: false });
  const expiresAt = info.data?.expiresAt;

  // The code stops working at `expiresAt`. This device's clock can be wrong, so reaching that time only asks the
  // server again: its 404 or 400 shows the expired screen, and a still valid code stays approvable.
  const refetchInfo = info.refetch;
  useEffect(() => {
    if (expiresAt === undefined) return;
    const timer = window.setTimeout(() => void refetchInfo(), Math.min(MAX_TIMER_MS, Math.max(0, expiresAt * 1000 - Date.now())));
    return () => window.clearTimeout(timer);
  }, [expiresAt, refetchInfo]);

  const gone = outcome?.kind === "gone" || (info.isError && isGone(info.error));

  if (outcome?.kind === "approved") {
    return (
      <AuthLayout>
        <PageHeader
          title="Desktop approved"
          description={
            outcome.replaced
              ? `${safeLabel(outcome.name)} now uses the new desktop key. Go back to Monoptah on your desktop to finish.`
              : "Go back to Monoptah on your desktop to finish. It connects within a few seconds."
          }
        />
        <Notice>
          {outcome.replaced
            ? "Devices that were linked to this desktop must be confirmed again on the desktop."
            : "New devices you add must be confirmed on the desktop before they can connect."}
        </Notice>
        <BackToDesktops />
      </AuthLayout>
    );
  }

  if (outcome?.kind === "denied") {
    return (
      <AuthLayout>
        <PageHeader title="Request denied" description="Nothing was added to your account. Your desktop will show that the request was denied." />
        <BackToDesktops />
      </AuthLayout>
    );
  }

  if (gone) {
    return (
      <AuthLayout>
        <PageHeader title="Code expired" description={PAIR_EXPIRED_MESSAGE} />
        <BackToDesktops />
      </AuthLayout>
    );
  }

  if (info.isError) {
    return (
      <AuthLayout>
        <PageHeader title="Approve this desktop" />
        <Notice variant="error">{pairErrorMessage(info.error)}</Notice>
        <Button type="button" size="lg" className="w-full" onClick={() => void info.refetch()}>
          Try again
        </Button>
      </AuthLayout>
    );
  }

  if (!info.data || desktops.isPending) {
    return (
      <AuthLayout>
        <PageHeader title="Approve this desktop" />
        <DelayedSpinner label="Loading the pairing request" />
      </AuthLayout>
    );
  }

  const pairing = info.data;
  const check: FingerprintCheck = checkFingerprint(hash, pairing.fingerprint);
  const [lineOne, lineTwo] = fingerprintLines(pairing.fingerprint);
  const existing = desktops.data?.desktops ?? [];
  const canApprove = check === "match" || (check === "missing" && comparedByEye);

  async function approve() {
    setBusy("approve");
    setError(null);
    setCancelled(false);
    const replaces = choice === NEW_DESKTOP ? undefined : choice;
    try {
      const result = await withFreshAuth(() => apiFetch(endpoints.pairApprove, { params: { userCode: code }, body: replaces === undefined ? {} : { replaces } }));
      void queryClient.invalidateQueries({ queryKey: desktopsQuery.queryKey });
      setOutcome({ kind: "approved", name: result.name, replaced: replaces !== undefined });
    } catch (caught) {
      if (caught instanceof ReauthCancelledError) setCancelled(true);
      else if (isApiError(caught, "conflict")) setOutcome({ kind: "gone" });
      else if (isApiError(caught, "not_found")) {
        // Either the code is gone or the chosen desktop is: the request itself tells which.
        const again = await info.refetch();
        if (again.isError) setOutcome({ kind: "gone" });
        else {
          setChoice(NEW_DESKTOP);
          void desktops.refetch();
          setError(PAIR_DESKTOP_GONE_MESSAGE);
        }
      } else setError(pairErrorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function deny() {
    setBusy("deny");
    setError(null);
    setCancelled(false);
    try {
      await apiFetch(endpoints.pairDeny, { params: { userCode: code } });
      setOutcome({ kind: "denied" });
    } catch (caught) {
      if (isGone(caught)) setOutcome({ kind: "gone" });
      else setError(pairErrorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  /** Cancel on a mismatch: the request is denied first (always safe, no passkey), then the page is left. */
  async function cancelMismatch() {
    setBusy("deny");
    setError(null);
    try {
      await apiFetch(endpoints.pairDeny, { params: { userCode: code } });
    } catch (caught) {
      // A code that is already gone needs no denial; anything else keeps the page so the user can try again.
      if (!isGone(caught)) {
        setError(pairErrorMessage(caught));
        setBusy(null);
        return;
      }
    }
    await navigate({ to: "/desktops" });
  }

  return (
    <AuthLayout>
      <PageHeader
        title={check === "mismatch" ? "Do not approve this desktop" : "Approve this desktop?"}
        description="Compare the confirmation code and fingerprint with the Monoptah window on your desktop."
      />

      {check === "mismatch" ? (
        <Notice variant="error" title="The fingerprint does not match">
          The link you opened carries a different fingerprint than the one Monoptah received for this request. The link may have been changed, or it belongs to another
          desktop. Do not approve it. Deny the request, then choose Pair on your desktop to make a new code.
        </Notice>
      ) : null}
      {check === "missing" ? (
        <Notice variant="error" title="This link has no fingerprint">
          Monoptah cannot check the desktop key for you. Before you approve, make sure the confirmation code and the fingerprint below are exactly what your desktop shows. If
          anything differs, deny.
        </Notice>
      ) : null}
      {check === "match" ? (
        <Notice title="The fingerprint matches your link">Still check that the confirmation code below is the one on your desktop.</Notice>
      ) : null}

      <dl className="flex flex-col gap-4 rounded-card bg-surface p-4">
        <Detail label="Confirmation code">
          <span className="block font-mono text-display font-semibold tracking-wider tabular-nums">{pairing.confirmationCode}</span>
        </Detail>
        <Detail label="Machine name">{safeLabel(pairing.machineName, 64, "Unnamed desktop")}</Detail>
        <Detail label="System">{safeLabel(pairing.os, 32, "Unknown")}</Detail>
        <Detail label="Fingerprint">
          <span className="block font-mono text-label tabular-nums">{lineOne}</span>
          <span className="block font-mono text-label tabular-nums">{lineTwo}</span>
        </Detail>
      </dl>

      {check !== "mismatch" && existing.length > 0 ? (
        <fieldset className="flex flex-col gap-1" disabled={busy !== null}>
          <legend className="mb-1 text-label font-medium text-muted-text">Add it as</legend>
          <ChoiceRow name="pair-target" value={NEW_DESKTOP} checked={choice === NEW_DESKTOP} onSelect={setChoice} label="A new desktop" />
          {existing.map((desktop) => (
            <ChoiceRow
              key={desktop.id}
              name="pair-target"
              value={desktop.id}
              checked={choice === desktop.id}
              onSelect={setChoice}
              label={`This replaces ${safeLabel(desktop.name, 64, "Unnamed desktop")}`}
            />
          ))}
          {choice !== NEW_DESKTOP ? (
            <p className="px-1 text-meta text-muted-text">
              The desktop keeps its name and settings, but its old key stops working and every device linked to it must be confirmed again on the desktop.
            </p>
          ) : null}
        </fieldset>
      ) : null}

      {check === "missing" ? (
        <Checkbox checked={comparedByEye} onChange={(event) => setComparedByEye(event.target.checked)} label="The code and fingerprint match my desktop screen" />
      ) : null}

      {error ? <Notice variant="error">{error}</Notice> : null}
      {cancelled ? <Notice>The passkey prompt was closed. Press Approve when you are ready.</Notice> : null}

      <div className="flex flex-col gap-2">
        {check !== "mismatch" ? (
          <>
            <Button type="button" size="lg" className="w-full" disabled={busy !== null || !canApprove} aria-busy={busy === "approve"} onClick={() => void approve()}>
              <Icon name="fingerprint" size={20} />
              Approve
              {busy === "approve" ? <DelayedSpinner label="Approving" /> : null}
            </Button>
            <p className="text-center text-meta text-muted-text">Approving asks for your passkey.</p>
          </>
        ) : null}
        <Button
          type="button"
          size="lg"
          variant={check === "mismatch" ? "default" : "outline"}
          className="w-full"
          disabled={busy !== null}
          aria-busy={busy === "deny"}
          onClick={() => void deny()}
        >
          Deny
          {busy === "deny" ? <DelayedSpinner label="Denying" /> : null}
        </Button>
        {check === "mismatch" ? (
          <Button type="button" size="lg" variant="outline" className="w-full" disabled={busy !== null} onClick={() => void cancelMismatch()}>
            Cancel
          </Button>
        ) : null}
      </div>
    </AuthLayout>
  );
}

function ChoiceRow({ name, value, checked, onSelect, label }: { name: string; value: string; checked: boolean; onSelect: (value: string) => void; label: string }) {
  return (
    <label className="flex min-h-11 cursor-pointer items-center gap-3 rounded-row px-2 text-body text-content has-checked:bg-selection">
      <Radio name={name} value={value} checked={checked} onChange={() => onSelect(value)} />
      <span className="min-w-0 flex-1 wrap-anywhere">{label}</span>
    </label>
  );
}
