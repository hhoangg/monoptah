// "Few recovery codes left" reminder (W-02, M17). Recovering with 2 or fewer codes left sets a boolean
// flag in sessionStorage (nothing secret). The Worker refuses regenerate for the recovery-only session, so
// the prompt comes after the next passkey sign-in in this tab: the auth guard sends the user to the forced
// regenerate on /settings until new codes were created.
const KEY = "monoptah.lowCodes";

/** The server asks for new codes when this many or fewer are left. */
export const LOW_CODES_THRESHOLD = 2;

export function setLowCodes(): void {
  try {
    sessionStorage.setItem(KEY, "1");
  } catch {
    // Storage blocked: the reminder is lost, the user is not blocked.
  }
}

export function hasLowCodes(): boolean {
  try {
    return sessionStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function clearLowCodes(): void {
  try {
    sessionStorage.removeItem(KEY);
  } catch {
    // Nothing to clear.
  }
}
