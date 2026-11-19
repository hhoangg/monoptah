// "Few recovery codes left" reminder (W-02, M17). Recovering with 2 or fewer codes left sets a boolean
// flag in sessionStorage (the account handle, nothing secret). The Worker refuses regenerate for the recovery-only session, so
// the prompt comes after the next passkey sign-in in this tab: the auth guard sends the user to the forced
// regenerate on /settings until new codes were created.
const KEY = "monoptah.lowCodes";

/** The server asks for new codes when this many or fewer are left. */
export const LOW_CODES_THRESHOLD = 2;

/** Stores the handle of the account that recovered (not secret), so another account is never prompted. */
export function setLowCodes(handle: string): void {
  try {
    sessionStorage.setItem(KEY, handle);
  } catch {
    // Storage blocked: the reminder is lost, the user is not blocked.
  }
}

export function clearLowCodes(): void {
  try {
    sessionStorage.removeItem(KEY);
  } catch {
    // Nothing to clear.
  }
}

/** True when the flag belongs to `handle`. A flag of another account is ignored and cleared. */
export function hasLowCodes(handle: string): boolean {
  try {
    const stored = sessionStorage.getItem(KEY);
    if (stored === null) return false;
    if (stored === handle) return true;
    clearLowCodes();
    return false;
  } catch {
    return false;
  }
}
