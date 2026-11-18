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
export type InviteName = "pairing" | "devices" | "ui" | "w14" | "w14b";

export function namedInvite(name: InviteName): string {
  const value = process.env[`E2E_INVITE_${name.toUpperCase()}`];
  if (!value) throw new Error(`E2E_INVITE_${name.toUpperCase()} is not set: the global setup did not run`);
  return value;
}
