// Small display helpers for the settings screens. Times from the API are unix seconds.

const DATE = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });

export function formatDate(seconds: number): string {
  return DATE.format(new Date(seconds * 1000));
}

/** "just now", "5 minutes ago", "3 days ago", else the date. `now` is unix seconds (injectable for tests). */
export function formatRelative(seconds: number, now = Math.floor(Date.now() / 1000)): string {
  const diff = Math.max(0, now - seconds);
  if (diff < 60) return "just now";
  const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;
  if (diff < 3600) return plural(Math.floor(diff / 60), "minute");
  if (diff < 86400) return plural(Math.floor(diff / 3600), "hour");
  if (diff < 30 * 86400) return plural(Math.floor(diff / 86400), "day");
  return formatDate(seconds);
}

/** A short "Browser on OS" label from a user agent string; the raw text when it is not recognised. */
export function describeUserAgent(userAgent: string | null): string {
  if (!userAgent) return "Unknown browser";
  const ua = userAgent;
  const browser = /Edg\//.test(ua) ? "Edge" : /OPR\/|Opera/.test(ua) ? "Opera" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\/|CriOS\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : null;
  const os = /iPhone|iPad|iPod/.test(ua) ? "iOS" : /Android/.test(ua) ? "Android" : /Windows/.test(ua) ? "Windows" : /Mac OS X|Macintosh/.test(ua) ? "macOS" : /CrOS/.test(ua) ? "ChromeOS" : /Linux/.test(ua) ? "Linux" : null;
  if (browser && os) return `${browser} on ${os}`;
  if (browser) return browser;
  return ua.length > 60 ? `${ua.slice(0, 57)}...` : ua;
}
