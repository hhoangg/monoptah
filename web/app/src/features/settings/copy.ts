// User-facing text of the settings screens. Errors reuse the account copy where it exists.
import { isApiError } from "@/features/accounts/api";
import { GENERIC_ERROR, NETWORK_ERROR } from "@/features/accounts/authCopy";

export const REAUTH_CANCELLED = "The passkey prompt was closed. Nothing was changed.";

/** One line for a failed settings call. */
export function settingsErrorMessage(error: unknown): string {
  if (isApiError(error, "network")) return NETWORK_ERROR;
  if (isApiError(error, "rate_limited")) return "Too many attempts. Wait a minute and try again.";
  if (isApiError(error, "last_passkey")) return "You cannot remove your only passkey. Add another one first.";
  if (isApiError(error, "invites_paused")) return "Invites are paused right now.";
  if (isApiError(error, "quota_exceeded")) return "You have no invites left.";
  return GENERIC_ERROR;
}
