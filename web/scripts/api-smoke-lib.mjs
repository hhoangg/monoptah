// Pure helpers of api-smoke.mjs, kept apart so they can be unit tested without starting a server.
export const SESSION_COOKIE = "__Host-mc_session";

/**
 * Reads the session cookie out of the `Set-Cookie` lines of one response.
 * Returns undefined when the response does not touch it, `{ token: null }` when it clears it,
 * and `{ token }` when it sets it.
 */
export function parseSessionCookie(setCookieLines) {
  for (const line of setCookieLines) {
    const [pair, ...attrs] = line.split(";").map((part) => part.trim());
    const eq = pair.indexOf("=");
    if (eq < 0 || pair.slice(0, eq) !== SESSION_COOKIE) continue;
    const value = pair.slice(eq + 1);
    const expired = attrs.some((a) => /^max-age=0$/i.test(a) || /^expires=.*1970/i.test(a));
    return { token: value === "" || expired ? null : value };
  }
  return undefined;
}

/** Secrets shorter than this are ignored by the log scan: a short string would match by chance. */
export const MIN_SECRET_LENGTH = 12;

/** Returns the labels (never the values) of every secret that appears in `text`. */
export function findSecretHits(text, secrets) {
  const hits = [];
  for (const [label, value] of secrets) {
    if (typeof value === "string" && value.length >= MIN_SECRET_LENGTH && text.includes(value)) hits.push(label);
  }
  return hits;
}

/** Replaces every known secret in `text` with `[redacted]`. */
export function redact(text, secrets) {
  let out = text;
  for (const [, value] of secrets) {
    if (typeof value === "string" && value.length >= MIN_SECRET_LENGTH) out = out.split(value).join("[redacted]");
  }
  return out;
}
