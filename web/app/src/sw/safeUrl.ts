// The notification click target (architecture.md 8, P-04). The payload `url` came through the Worker,
// but the service worker still treats it as untrusted: only a same-origin relative path under `/d/` (or `/settings`) may
// open a window. The rules mirror features/accounts/redirect.ts (`safeRedirectTarget`); the logic is copied
// because that module also touches sessionStorage, which a worker does not have, and the worker bundle
// must not pull in app code.

/** Resolved against this base to prove a target stays on the current origin. */
const BASE = "https://app.invalid";
const ALLOWED_PREFIX = "/d/";
/** The one other page a notification may open: the test notification of Settings sends `/settings`. */
const SETTINGS_PATH = "/settings";
const MAX_URL_LENGTH = 2048;

/** What a click opens when there is no usable URL: the app root, which routes to the right place. */
export const FALLBACK_URL = "/";

/** Returns `/d/...` or `/settings` (path, query and hash) when `raw` is a safe same-origin relative path, else null. */
export function safeNotificationUrl(raw: unknown): string | null {
  if (typeof raw !== "string" || raw.length === 0 || raw.length > MAX_URL_LENGTH) return null;
  const isSettings = raw === SETTINGS_PATH || raw.startsWith(`${SETTINGS_PATH}?`) || raw.startsWith(`${SETTINGS_PATH}#`);
  if (!raw.startsWith(ALLOWED_PREFIX) && !isSettings) return null;
  // Control characters, tabs and backslashes are normalised by URL parsers in ways that can turn a
  // relative path into an absolute one.
  for (const char of raw) {
    const code = char.charCodeAt(0);
    if (code <= 0x1f || code === 0x7f || char === "\\") return null;
  }
  let url: URL;
  try {
    url = new URL(raw, BASE);
  } catch {
    return null;
  }
  if (url.origin !== BASE) return null;
  // Dot segments collapse during parsing ("/d/../../evil" becomes "/evil"): the result must still be under /d/.
  if (isSettings ? url.pathname !== SETTINGS_PATH : !url.pathname.startsWith(ALLOWED_PREFIX)) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}

/** The URL a click opens: the validated payload URL, else the app root. */
export function clickTarget(raw: unknown): string {
  return safeNotificationUrl(raw) ?? FALLBACK_URL;
}
