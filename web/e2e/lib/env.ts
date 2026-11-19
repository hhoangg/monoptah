export function origin(): string {
  const value = process.env["E2E_ORIGIN"];
  if (!value) throw new Error("E2E_ORIGIN is not set: the global setup did not run");
  return value;
}

/** The one-use bootstrap invite code. Never print it. */
export function bootstrapInvite(): string {
  const value = process.env["E2E_INVITE"];
  if (!value) throw new Error("E2E_INVITE is not set: the global setup did not run");
  return value;
}

/** Named bootstrap invites minted by the global setup: one per spec that needs its own account. Never print them. */
export type InviteName = "pairing" | "devices" | "ui" | "r12" | "w14" | "w14b" | "filepreview";

export function namedInvite(name: InviteName): string {
  const value = process.env[`E2E_INVITE_${name.toUpperCase()}`];
  if (!value) throw new Error(`E2E_INVITE_${name.toUpperCase()} is not set: the global setup did not run`);
  return value;
}

/** Restarts the local Worker (wrangler dev) on the same port and persistence. Resolves when it answers again. */
export async function restartRelay(): Promise<void> {
  const control = process.env["E2E_CONTROL"];
  if (!control) throw new Error("E2E_CONTROL is not set: the global setup did not run");
  const res = await fetch(`${control}/restart`, { method: "POST", signal: AbortSignal.timeout(240_000) });
  if (res.status !== 200) throw new Error(`relay restart failed: ${res.status} ${await res.text()}`);
}

/**
 * Harness only: empties the local rate limiter table (D1 rows of `rate_limits`) and restarts the Worker (which also
 * drops its in-memory per-IP pre-filter) so the next spec file starts a fresh window. Every headless `pair` comes from one address and `pair/start` allows 5 per 600 s, fewer than the suite pairs.
 * Call it from a `beforeAll` of each spec that runs a headless `pair`. It touches no Worker code and no server switch.
 */
export async function clearRateLimits(): Promise<void> {
  const control = process.env["E2E_CONTROL"];
  if (!control) throw new Error("E2E_CONTROL is not set: the global setup did not run");
  const res = await fetch(`${control}/clear-rate-limits`, { method: "POST", signal: AbortSignal.timeout(240_000) });
  if (res.status !== 200) throw new Error(`clearing the rate limits failed: ${res.status} ${await res.text()}`);
}
