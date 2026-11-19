// WebAuthn helpers over @simplewebauthn/server (security.md 3.1), shared by registration, login and re-auth (A-03)
// and by "add passkey" (A-08). Every ceremony requires user verification and a discoverable credential, asks for
// no attestation and leaves `authenticatorAttachment` unset. Expected origin and RP ID always come from the
// environment (`RP_ORIGIN`, `RP_ID`), never from the request.
import {
  generateAuthenticationOptions,
  generateRegistrationOptions,
  verifyAuthenticationResponse,
  verifyRegistrationResponse,
  type AuthenticationResponseJSON,
  type PublicKeyCredentialCreationOptionsJSON,
  type PublicKeyCredentialRequestOptionsJSON,
  type RegistrationResponseJSON,
} from "@simplewebauthn/server";
import type { Env } from "../../env.ts";

export const RP_NAME = "Monoptah";

type WebAuthnEnv = Pick<Env, "RP_ID" | "RP_ORIGIN">;

export interface CredentialRef {
  /** Base64url credential id (`passkeys.id`). */
  id: string;
  /** JSON array text of transports (`passkeys.transports`), or already parsed. */
  transports?: string[];
}

/** The WebAuthn user handle of a user: the UTF-8 bytes of the user id (a UUID, so 36 bytes, under the 64 byte limit). */
export function webauthnUserHandle(userId: string): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(userId) as Uint8Array<ArrayBuffer>;
}

export function toBase64UrlString(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export interface RegistrationOptionsInput {
  userId: string;
  /** The handle: the account name the browser shows. */
  userName: string;
  displayName: string;
  /** Passkeys the user already has, so the same authenticator is not registered twice. */
  excludeCredentials?: CredentialRef[];
}

/** Creation options: discoverable credential and user verification required, no attestation, any attachment. */
export function buildRegistrationOptions(env: WebAuthnEnv, input: RegistrationOptionsInput): Promise<PublicKeyCredentialCreationOptionsJSON> {
  return generateRegistrationOptions({
    rpName: RP_NAME,
    rpID: env.RP_ID,
    userName: input.userName,
    userDisplayName: input.displayName,
    userID: webauthnUserHandle(input.userId),
    attestationType: "none",
    authenticatorSelection: { residentKey: "required", userVerification: "required" },
    excludeCredentials: input.excludeCredentials,
  });
}

export interface VerifiedRegistration {
  credentialId: string;
  publicKey: Uint8Array;
  counter: number;
  transports: string[];
  deviceType: "singleDevice" | "multiDevice";
  backedUp: boolean;
}

/** The longest credential id WebAuthn allows (Credential ID, 1023 bytes). */
export const MAX_CREDENTIAL_ID_BYTES = 1023;
/** At most this many transports are kept per passkey, each at most `MAX_TRANSPORT_LENGTH` characters. */
export const MAX_TRANSPORTS = 8;
export const MAX_TRANSPORT_LENGTH = 32;

/** The decoded length of a base64url string, without decoding it. */
export function base64urlByteLength(text: string): number {
  const length = text.replace(/=+$/, "").length;
  return Math.floor((length * 3) / 4);
}

/** The transports an authenticator reports, bounded: at most 8 entries of at most 32 characters (the rest is dropped). */
export function boundedTransports(values: readonly unknown[]): string[] {
  return values
    .map(String)
    .filter((value) => value.length > 0 && value.length <= MAX_TRANSPORT_LENGTH)
    .slice(0, MAX_TRANSPORTS);
}

/** Verifies an attestation against the stored challenge. Returns null for anything that does not verify. */
export async function verifyRegistration(env: WebAuthnEnv, response: unknown, expectedChallenge: string): Promise<VerifiedRegistration | null> {
  try {
    const verified = await verifyRegistrationResponse({
      response: response as RegistrationResponseJSON,
      expectedChallenge,
      expectedOrigin: env.RP_ORIGIN,
      expectedRPID: env.RP_ID,
      requireUserPresence: true,
      requireUserVerification: true,
    });
    if (!verified.verified) return null;
    const { credential, credentialDeviceType, credentialBackedUp } = verified.registrationInfo;
    // Refused before any write: a longer id is not valid WebAuthn and would only bloat the row and the audit meta.
    if (base64urlByteLength(credential.id) > MAX_CREDENTIAL_ID_BYTES) return null;
    return {
      credentialId: credential.id,
      publicKey: credential.publicKey,
      counter: credential.counter,
      transports: boundedTransports((response as RegistrationResponseJSON).response.transports ?? credential.transports ?? []),
      deviceType: credentialDeviceType,
      backedUp: credentialBackedUp,
    };
  } catch {
    return null;
  }
}

/** Request options: user verification required. No `allowCredentials` makes the ceremony discoverable (login). */
export function buildAuthenticationOptions(env: WebAuthnEnv, allowCredentials?: CredentialRef[]): Promise<PublicKeyCredentialRequestOptionsJSON> {
  return generateAuthenticationOptions({
    rpID: env.RP_ID,
    userVerification: "required",
    allowCredentials: allowCredentials && allowCredentials.length > 0 ? allowCredentials : undefined,
  });
}

export interface StoredCredential {
  id: string;
  publicKey: Uint8Array;
  counter: number;
  transports?: string[];
}

export type AuthenticationResult =
  | { ok: true; newCounter: number; backedUp: boolean; deviceType: "singleDevice" | "multiDevice" }
  /** The assertion does not verify (signature, challenge, origin, RP ID, user verification, user handle). */
  | { ok: false; reason: "invalid" }
  /** The signature counter did not increase on an authenticator that uses counters (security.md 3.1). */
  | { ok: false; reason: "counter" };

/**
 * The counter rule of security.md 3.1: a counter that does not increase is refused, unless the authenticator
 * does not use counters (stored 0 and reported 0 stay accepted).
 */
export function counterDecreased(stored: number, reported: number): boolean {
  return (reported > 0 || stored > 0) && reported <= stored;
}

/**
 * Verifies an assertion against the stored challenge and credential, then applies the counter rule here (the
 * library is given counter 0 so its own check never throws and the caller can tell a refused counter from a bad
 * signature). When `expectedUserId` is given and the response carries a user handle, the handle must match.
 */
export async function verifyAuthentication(
  env: WebAuthnEnv,
  response: unknown,
  expectedChallenge: string,
  credential: StoredCredential,
  expectedUserId?: string,
): Promise<AuthenticationResult> {
  const typed = response as AuthenticationResponseJSON;
  if (expectedUserId !== undefined && typeof typed.response?.userHandle === "string") {
    if (typed.response.userHandle !== toBase64UrlString(webauthnUserHandle(expectedUserId))) return { ok: false, reason: "invalid" };
  }
  let verified;
  try {
    verified = await verifyAuthenticationResponse({
      response: typed,
      expectedChallenge,
      expectedOrigin: env.RP_ORIGIN,
      expectedRPID: env.RP_ID,
      credential: { id: credential.id, publicKey: credential.publicKey as Uint8Array<ArrayBuffer>, counter: 0, transports: credential.transports },
      requireUserVerification: true,
    });
  } catch {
    return { ok: false, reason: "invalid" };
  }
  if (!verified.verified) return { ok: false, reason: "invalid" };
  const info = verified.authenticationInfo;
  if (counterDecreased(credential.counter, info.newCounter)) return { ok: false, reason: "counter" };
  return { ok: true, newCounter: info.newCounter, backedUp: info.credentialBackedUp, deviceType: info.credentialDeviceType };
}
