// User-facing text for the sign-in family of screens. Invite failures are deliberately one message
// (security.md 3: no oracle for which invites exist).
import { isApiError } from "./api";

export const INVITE_ERROR = "This invite is not valid. Ask the person who invited you for a new link.";
export const GENERIC_ERROR = "Something went wrong. Try again.";
export const NETWORK_ERROR = "Cannot reach Monoptah. Check your connection and try again.";
export const RECOVER_ERROR = "That handle or recovery code is not valid.";
export const UNSUPPORTED_ERROR = "This browser cannot use passkeys. Try a recent version of Safari, Chrome, Edge or Firefox.";

function retryText(seconds: number | null): string {
  if (seconds === null) return "Too many attempts. Wait a moment and try again.";
  return seconds <= 60 ? "Too many attempts. Wait a minute and try again." : `Too many attempts. Try again in ${Math.ceil(seconds / 60)} minutes.`;
}

function common(error: unknown): string {
  if (isApiError(error, "network")) return NETWORK_ERROR;
  if (isApiError(error, "rate_limited")) return retryText(error.retryAfter);
  return GENERIC_ERROR;
}

export function registerErrorMessage(error: unknown): string {
  if (isApiError(error)) {
    // Every invite failure reads the same: invalid, expired, used, paused, or the generic code.
    if (error.code === "invalid_invite" || error.code === "invites_paused" || error.code === "quota_exceeded") return INVITE_ERROR;
    if (error.code === "conflict") return "That handle or passkey is already registered. Pick another handle, or sign in if this is your account.";
    if (error.code === "invalid") return "Check the handle and name, then try again.";
  }
  return common(error);
}

export function signInErrorMessage(error: unknown): string {
  if (isApiError(error, "unauthorized")) return "That passkey was not recognised. Try again or use a recovery code.";
  if (isApiError(error, "blocked")) return "This account cannot sign in.";
  return common(error);
}

/** One answer for every recover failure (unknown handle, wrong code, used code). */
export function recoverErrorMessage(error: unknown): string {
  if (isApiError(error, "network") || isApiError(error, "rate_limited")) return common(error);
  if (isApiError(error) && (error.status === 401 || error.status === 400 || error.status === 404)) return RECOVER_ERROR;
  return common(error);
}

export function passkeyErrorMessage(error: unknown): string {
  if (isApiError(error, "conflict")) return "This device already has a passkey for your account.";
  return common(error);
}
