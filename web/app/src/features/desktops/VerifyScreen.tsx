// `/desktops/:id/verify#fp=<64 hex>` (R-14d, security.md 5): the only place that replaces a desktop pin.
// Owner: WS-M. The desktop shows this link (and a QR) under Settings > Web app > "Verify on another
// device". The `#fp` fragment never reaches the server. This page fetches the desktop's directory key
// fresh and checks that the SHA-256 of that key equals the full fingerprint of the link. Even then it
// does NOT re-pin on load (R13A-1): a link can come from an attacker, so the page shows the fingerprint,
// and the pin is replaced only after the user ticks "matches my desktop" and clicks "Trust this key".
// The key is fetched again at the click. Anything else (a mismatch, a missing or malformed fragment)
// keeps the old pin and offers no way to accept.
//
// The fragment survives a sign-in because the root auth guard parks it in sessionStorage before the
// redirect to /login and `postSignInTarget` puts it back (features/accounts/redirect.ts); the page also
// reads a parked value itself when the address carries none. After reading, the fragment is removed
// from the address bar with `history.replaceState`.
import { useRouterState } from "@tanstack/react-router";
import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { apiFetch, isApiError } from "@/features/accounts/api";
import { fingerprintFromHash } from "@/features/accounts/pair/fingerprint";
import { restoreFingerprintFragment } from "@/features/accounts/redirect";
import { SettingsLayout } from "@/features/settings/SettingsLayout";
import { base64UrlDecode } from "@/link/base64url";
import { fingerprint, normalizeFingerprint } from "@/link/fingerprint";
import { getLinkConnection } from "@/link";
import { repinFromVerify } from "@/link/pins";
import { Button, buttonVariants } from "@/ui/button";
import { Checkbox } from "@/ui/Checkbox";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { desktopEndpoints } from "./api";
import { safeName } from "./format";
import { Fingerprint } from "./parts";

type Outcome =
  | { kind: "checking" }
  | { kind: "missing" }
  | { kind: "invalid" }
  | { kind: "confirm"; name: string; fingerprint: string }
  | { kind: "verified"; name: string; fingerprint: string }
  | { kind: "mismatch" }
  | { kind: "not_found" }
  | { kind: "error" };

const PUBLIC_KEY_BYTES = 32;

/**
 * The raw fragment value, from the address or from the value parked across a sign-in. Reading the parked
 * value consumes it (a side effect), so this runs in an effect, never during render. It is always read,
 * so a leftover parked value is cleared even when the address already carries `#fp`.
 */
function readFragment(hash: string, pathname: string): string | null {
  const parked = restoreFingerprintFragment(pathname);
  const fromHash = fingerprintFromHash(hash);
  if (fromHash !== null) return fromHash;
  return parked === null ? null : fingerprintFromHash(parked);
}

function stripFragment(): void {
  const { pathname, search } = window.location;
  window.history.replaceState(window.history.state, "", `${pathname}${search}`);
}

export function VerifyScreen({ id }: { id: string }) {
  const hash = useRouterState({ select: (state) => state.location.hash });
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  // `undefined` until the mount effect read the fragment; then the value or null. Read once: the fragment
  // is removed from the address bar right after.
  const [fragment, setFragment] = useState<string | null | undefined>(undefined);
  const resolved = useRef<{ value: string | null } | null>(null);
  const [outcome, setOutcome] = useState<Outcome>({ kind: "checking" });
  const [attempt, setAttempt] = useState(0);
  const started = useRef<number | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    // The ref keeps the first read when strict mode runs this effect twice (the parked value is consumed once).
    resolved.current ??= { value: readFragment(hash, pathname) };
    const { value } = resolved.current;
    setFragment(value);
    if (value === null) setOutcome({ kind: "missing" });
    stripFragment();
    // Reruns after the fragment is stripped from the address; the ref keeps the first read.
  }, [hash, pathname]);

  useEffect(() => {
    if (fragment === undefined || fragment === null || started.current === attempt) return;
    started.current = attempt;
    void (async () => {
      const result = await inspect(id, fragment);
      if (alive.current) setOutcome(result);
    })();
  }, [id, fragment, attempt]);

  async function confirm(shown: string) {
    // Re-fetches the key: the click acts on the key the server serves now, and only when it still hashes to
    // the fingerprint the user compared.
    const result = await pin(id, shown);
    if (alive.current) setOutcome(result);
  }

  return (
    <SettingsLayout>
      <PageHeader title="Verify desktop" description={describe(outcome)} />
      <Body id={id} outcome={outcome} onConfirm={confirm} onRetry={() => { setOutcome({ kind: "checking" }); setAttempt((count) => count + 1); }} />
    </SettingsLayout>
  );
}

function describe(outcome: Outcome): string {
  switch (outcome.kind) {
    case "checking":
      return "Checking the link against the key of your desktop.";
    case "confirm":
      return "Compare this fingerprint with the one on your desktop before you trust it.";
    case "verified":
      return "This device now trusts the key of this desktop.";
    case "mismatch":
      return "The link and the desktop disagree.";
    default:
      return "Confirm a desktop key that changed, using a link from the desktop itself.";
  }
}

type Loaded = { outcome: Outcome } | { name: string; key: Uint8Array };

async function loadKey(id: string): Promise<Loaded> {
  let directory;
  try {
    directory = await apiFetch(desktopEndpoints.get, { params: { id } });
  } catch (error) {
    return { outcome: isApiError(error, "not_found") || isApiError(error, "forbidden") ? { kind: "not_found" } : { kind: "error" } };
  }
  let key: Uint8Array;
  try {
    key = base64UrlDecode(directory.publicKey);
  } catch {
    return { outcome: { kind: "error" } };
  }
  if (key.length !== PUBLIC_KEY_BYTES) return { outcome: { kind: "error" } };
  return { name: safeName(directory.name, "Unnamed desktop"), key };
}

/** Read only: checks the link against the directory key and never writes the pin. */
async function inspect(id: string, fragment: string): Promise<Outcome> {
  const loaded = await loadKey(id);
  if ("outcome" in loaded) return loaded.outcome;
  const expected = normalizeFingerprint(fragment);
  if (!expected) return { kind: "invalid" };
  const actual = fingerprint(loaded.key);
  if (actual !== expected) return { kind: "mismatch" };
  return { kind: "confirm", name: loaded.name, fingerprint: actual };
}

/** The explicit action: replaces the pin with the key whose fingerprint the user compared. */
async function pin(id: string, shownFingerprint: string): Promise<Outcome> {
  const loaded = await loadKey(id);
  if ("outcome" in loaded) return loaded.outcome;
  const result = await repinFromVerify(id, shownFingerprint, loaded.key);
  if (!result.ok) return result.reason === "mismatch" ? { kind: "mismatch" } : { kind: "invalid" };
  // A running connection that stopped on `key_changed` restarts with the new pin; otherwise a no-op.
  getLinkConnection(id).reconnect();
  return { kind: "verified", name: loaded.name, fingerprint: result.fingerprint };
}

function ConfirmKey({ name, fingerprint: print, onConfirm }: { name: string; fingerprint: string; onConfirm: (fingerprint: string) => Promise<void> }) {
  const [compared, setCompared] = useState(false);
  const [busy, setBusy] = useState(false);
  return (
    <div className="flex flex-col gap-4">
      <Notice title="Check the fingerprint">
        The key of {name} has this fingerprint. Open Settings, then Web app on your desktop and compare it, group by group:
        <Fingerprint hex={print} className="mt-1 block text-body" />
      </Notice>
      <Checkbox checked={compared} onChange={(event) => setCompared(event.target.checked)} label="This matches Settings > Web app on my desktop" />
      <div>
        <Button
          type="button"
          size="lg"
          disabled={!compared || busy}
          onClick={() => {
            setBusy(true);
            void onConfirm(print).finally(() => setBusy(false));
          }}
        >
          Trust this key
        </Button>
      </div>
    </div>
  );
}

function Body({ id, outcome, onConfirm, onRetry }: { id: string; outcome: Outcome; onConfirm: (fingerprint: string) => Promise<void>; onRetry: () => void }) {
  const back = (
    <Link to="/settings/desktops/$id" params={{ id }} className={buttonVariants({ variant: "outline" })}>
      Manage this desktop
    </Link>
  );
  switch (outcome.kind) {
    case "checking":
      return (
        <div className="flex min-h-13 items-center justify-center py-8">
          <DelayedSpinner label="Checking the link" />
        </div>
      );
    case "confirm":
      return <ConfirmKey name={outcome.name} fingerprint={outcome.fingerprint} onConfirm={onConfirm} />;
    case "verified":
      return (
        <div className="flex flex-col gap-4">
          <Notice title="Desktop verified">
            The key of {outcome.name} matches the fingerprint in your link. Compare it with the one on your desktop:
            <Fingerprint hex={outcome.fingerprint} className="mt-1 block text-body" />
          </Notice>
          <div>
            <Link to="/d/$desktopId" params={{ desktopId: id }} className={buttonVariants({ size: "lg" })}>
              Open desktop
            </Link>
          </div>
        </div>
      );
    case "mismatch":
      return (
        <div className="flex flex-col gap-4">
          <Notice variant="error" title="Do not continue">
            This link does not match the desktop's key. Do not continue.
          </Notice>
          <p className="text-small text-muted-text">Your device keeps the key it trusted before. If you did not expect this, someone may be pretending to be your desktop. Make a new link on the desktop itself and try again.</p>
          <div>{back}</div>
        </div>
      );
    case "invalid":
    case "missing":
      return (
        <div className="flex flex-col gap-4">
          <Notice variant="error" title="This link is incomplete">
            It has no complete fingerprint, so the desktop key cannot be checked.
          </Notice>
          <p className="text-small text-muted-text">On your desktop, open Settings, then Web app, then "Verify on another device". Open that link, or scan its QR code, on this device. Do not type the address by hand.</p>
          <div>{back}</div>
        </div>
      );
    case "not_found":
      return (
        <div className="flex flex-col gap-4">
          <Notice variant="error">This desktop was not found on your account. It may have been revoked.</Notice>
          <div>
            <Link to="/settings/desktops" className={buttonVariants({ variant: "outline" })}>
              Go to desktops
            </Link>
          </div>
        </div>
      );
    case "error":
      return (
        <div className="flex flex-col gap-4">
          <Notice variant="error">The desktop key could not be loaded. Check your connection and try again.</Notice>
          <div>
            <Button type="button" variant="outline" onClick={onRetry}>
              Try again
            </Button>
          </div>
        </div>
      );
  }
}
