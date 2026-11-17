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

/** A second bootstrap invite (own account for the pairing spec). Never print it. */
export function pairingInvite(): string {
  const value = process.env["E2E_INVITE_PAIR"];
  if (!value) throw new Error("E2E_INVITE_PAIR is not set: the global setup did not run");
  return value;
}
