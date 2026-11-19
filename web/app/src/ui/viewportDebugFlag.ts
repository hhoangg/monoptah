// TEMPORARY: enable flag of the viewport debug overlay (kept apart so the overlay stays lazy).
const FLAG_KEY = "monoptah.debug.viewport";

/** Reads `?debug=viewport` / `?debug=off` once and the sessionStorage flag. Never throws. */
export function viewportDebugEnabled(search: string = window.location.search): boolean {
  try {
    const value = new URLSearchParams(search).get("debug");
    if (value === "viewport") window.sessionStorage.setItem(FLAG_KEY, "1");
    else if (value === "off") window.sessionStorage.removeItem(FLAG_KEY);
    return window.sessionStorage.getItem(FLAG_KEY) === "1";
  } catch {
    return false;
  }
}


/** Turns the flag on or off for this tab (sessionStorage); the caller reloads. For the installed app, which has no URL bar. */
export function setViewportDebug(on: boolean): void {
  try {
    if (on) window.sessionStorage.setItem(FLAG_KEY, "1");
    else window.sessionStorage.removeItem(FLAG_KEY);
  } catch {
    // Storage unavailable: the overlay simply stays off.
  }
}

/** TEMPORARY hidden palette command: typing exactly "debug viewport" or "debug off" toggles the overlay. */
export function viewportDebugCommand(query: string): boolean | null {
  const text = query.trim().toLowerCase();
  return text === "debug viewport" ? true : text === "debug off" ? false : null;
}
