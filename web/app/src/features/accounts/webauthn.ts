// Thin wrapper over `@simplewebauthn/browser` (security.md 3.1). The options JSON comes from the
// server; this module only runs the ceremony and turns a user cancel into a typed result.
import {
  browserSupportsWebAuthn,
  startAuthentication,
  startRegistration,
  type AuthenticationResponseJSON,
  type PublicKeyCredentialCreationOptionsJSON,
  type PublicKeyCredentialRequestOptionsJSON,
  type RegistrationResponseJSON,
} from "@simplewebauthn/browser";

export type CeremonyResult<T> = { status: "ok"; response: T } | { status: "cancelled" };

/** True when the browser can run a WebAuthn ceremony at all. */
export function passkeysSupported(): boolean {
  return browserSupportsWebAuthn();
}

function isCancel(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  if (error.name === "NotAllowedError" || error.name === "AbortError") return true;
  const code = (error as { code?: unknown }).code;
  if (code === "ERROR_CEREMONY_ABORTED") return true;
  // The library wraps the DOM error; the original is on `cause`.
  const cause = (error as { cause?: unknown }).cause;
  return cause instanceof Error && (cause.name === "NotAllowedError" || cause.name === "AbortError");
}

/** Creates a passkey. `cancelled` covers the user dismissing the prompt and a timeout. */
export async function createPasskey(options: { challenge: string }): Promise<CeremonyResult<RegistrationResponseJSON>> {
  try {
    const response = await startRegistration({ optionsJSON: options as PublicKeyCredentialCreationOptionsJSON });
    return { status: "ok", response };
  } catch (error) {
    if (isCancel(error)) return { status: "cancelled" };
    throw error;
  }
}

/**
 * Signs in with a passkey. Login options carry no `allowCredentials` (discoverable credentials),
 * so the browser also offers its hybrid QR option; this function passes the server options through.
 */
export async function getPasskey(options: { challenge: string }): Promise<CeremonyResult<AuthenticationResponseJSON>> {
  try {
    const response = await startAuthentication({ optionsJSON: options as PublicKeyCredentialRequestOptionsJSON });
    return { status: "ok", response };
  } catch (error) {
    if (isCancel(error)) return { status: "cancelled" };
    throw error;
  }
}
