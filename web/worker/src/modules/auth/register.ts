// Registration (A-03 passkey ceremony, A-06 invite gate): `register/options` validates the invite and handle and
// stores a challenge; `register/verify` verifies the attestation and creates the user, passkey, recovery codes,
// session and audit rows in ONE batch whose every statement is conditional on the first one (the user insert),
// which itself re-checks the invite, the pause switch and the user cap.
import type { Handler } from "hono";
import { RegisterOptionsBody, RegisterOptionsResponse, RegisterVerifyBody, RegisterVerifyResponse } from "@monoptah/protocol";
import { z } from "zod";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { auditStatement } from "../audit/audit.ts";
import { gateParams, GATE_SQL, registrationOpen } from "../invites/gate.ts";
import { hashInviteCode } from "../invites/code.ts";
import { consumeChallenge, storeChallenge } from "../passkeys/challenges.ts";
import { buildRegistrationOptions, verifyRegistration } from "../passkeys/webauthn.ts";
import { limitInviteAttempts } from "../ratelimit/index.ts";
import { generateRecoveryCodes } from "../recovery/codes.ts";
import { createSession } from "../sessions/session.ts";

/** A valid invite: it exists, is unused, not revoked and not expired. */
export const INVITE_LOOKUP_SQL = "SELECT id FROM invites WHERE code_hash = ? AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?";
export const HANDLE_TAKEN_SQL = "SELECT 1 AS taken FROM users WHERE handle = ?";
/**
 * The first statement of the registration batch. The role and the inviter come from the invite row itself
 * (a `bootstrap` invite makes an admin with no inviter), and the row is written only while the invite is still
 * valid and the gate (pause switch, user cap) is open. `changes = 0` means one of those failed.
 */
export const INSERT_USER_SQL = `INSERT INTO users (id, handle, display_name, role, status, invited_by, invite_id, created_at)
  SELECT ?, ?, ?, CASE i.kind WHEN 'bootstrap' THEN 'admin' ELSE 'user' END, 'active', i.created_by, i.id, ?
  FROM invites i WHERE i.id = ? AND i.code_hash = ? AND i.used_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > ? AND ${GATE_SQL}`;
export const USE_INVITE_SQL =
  "UPDATE invites SET used_by = ?, used_at = ? WHERE id = ? AND used_at IS NULL AND EXISTS (SELECT 1 FROM users WHERE id = ? AND invite_id = ?)";
export const INSERT_PASSKEY_SQL = `INSERT INTO passkeys (id, user_id, public_key, counter, transports, device_type, backed_up, name, created_at)
  SELECT ?, ?, ?, ?, ?, ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM users WHERE id = ? AND invite_id = ?)`;
export const INSERT_RECOVERY_CODE_SQL = `INSERT INTO recovery_codes (id, user_id, code_hash, batch, created_at)
  SELECT ?, ?, ?, 1, ? WHERE EXISTS (SELECT 1 FROM users WHERE id = ? AND invite_id = ?)`;

export const USER_ROLE_SQL = "SELECT role FROM users WHERE id = ?";

export const DEFAULT_PASSKEY_NAME = "Passkey";

const RegisterPayload = z.object({
  inviteId: z.string(),
  inviteCodeHash: z.string(),
  handle: z.string(),
  displayName: z.string(),
  /** Becomes `users.id` and, as UTF-8 bytes, the WebAuthn user handle. */
  userId: z.string(),
});

/** The one answer for every invite or gate failure: a bad code must not reveal why it is bad. */
function invalidInvite(c: AppContext): Response {
  return apiError(c, 400, "invalid_invite", "This invitation is not valid");
}

export const registerOptions: Handler<AppEnv> = async (c) => {
  const body = RegisterOptionsBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid registration request");
  const { invite, handle, displayName } = body.data;

  const limited = await limitInviteAttempts(c, invite);
  if (limited) return limited;

  const inviteCodeHash = await hashInviteCode(invite);
  const [row, open] = await Promise.all([
    c.env.DB.prepare(INVITE_LOOKUP_SQL).bind(inviteCodeHash, nowSeconds()).first<{ id: string }>(),
    registrationOpen(c.env),
  ]);
  if (!row || !open) return invalidInvite(c);
  if (await c.env.DB.prepare(HANDLE_TAKEN_SQL).bind(handle).first()) return apiError(c, 409, "conflict", "Handle already taken");

  const userId = crypto.randomUUID();
  const options = await buildRegistrationOptions(c.env, { userId, userName: handle, displayName });
  const challengeId = await storeChallenge(c.env, {
    purpose: "register",
    challenge: options.challenge,
    payload: { inviteId: row.id, inviteCodeHash, handle, displayName, userId } satisfies z.input<typeof RegisterPayload>,
  });
  return respond(c, RegisterOptionsResponse, { challengeId, options: { ...options } });
};

function isUniqueViolation(error: unknown): boolean {
  return error instanceof Error && /UNIQUE constraint failed/i.test(error.message);
}

export const registerVerify: Handler<AppEnv> = async (c) => {
  const body = RegisterVerifyBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid registration request");

  const challenge = await consumeChallenge(c.env, { id: body.data.challengeId, purpose: "register" });
  if (!challenge) return apiError(c, 400, "invalid", "Challenge expired or already used");
  const payload = RegisterPayload.safeParse(challenge.payload);
  if (!payload.success) return apiError(c, 400, "invalid", "Challenge expired or already used");
  const { inviteId, inviteCodeHash, handle, displayName, userId } = payload.data;

  const credential = await verifyRegistration(c.env, body.data.response, challenge.challenge);
  if (!credential) return apiError(c, 400, "invalid", "Registration could not be verified");

  const now = nowSeconds();
  const exists = { sql: "EXISTS (SELECT 1 FROM users WHERE id = ? AND invite_id = ?)", params: [userId, inviteId] };
  const recovery = await generateRecoveryCodes(c.env);
  const session = await createSession(c.env, {
    userId,
    amr: "passkey",
    userAgent: c.req.header("User-Agent"),
    reauthAt: now,
    now,
    when: exists,
  });
  const ip = await ipHash(c.env, clientIp(c.req.raw));
  const publicKey = credential.publicKey.slice().buffer;

  const statements = [
    c.env.DB.prepare(INSERT_USER_SQL).bind(userId, handle, displayName, now, inviteId, inviteCodeHash, now, ...gateParams(c.env)),
    c.env.DB.prepare(USE_INVITE_SQL).bind(userId, now, inviteId, userId, inviteId),
    c.env.DB.prepare(INSERT_PASSKEY_SQL).bind(
      credential.credentialId,
      userId,
      publicKey,
      credential.counter,
      JSON.stringify(credential.transports),
      credential.deviceType,
      credential.backedUp ? 1 : 0,
      DEFAULT_PASSKEY_NAME,
      now,
      userId,
      inviteId,
    ),
    ...recovery.hashes.map((hash) => c.env.DB.prepare(INSERT_RECOVERY_CODE_SQL).bind(crypto.randomUUID(), userId, hash, now, userId, inviteId)),
    session.statement,
    auditStatement(
      c.env,
      { action: "account.register", meta: { inviteId, ipHash: ip }, actorUserId: userId, targetType: "user", targetId: userId, ts: now },
      exists,
    ),
    auditStatement(c.env, { action: "invite.use", meta: { inviteId }, actorUserId: userId, targetType: "invite", targetId: inviteId, ts: now }, exists),
  ];

  let results: D1Result[];
  try {
    results = await c.env.DB.batch(statements);
  } catch (error) {
    // The handle (or the credential id) was taken between options and verify; the batch rolled back.
    if (isUniqueViolation(error)) return apiError(c, 409, "conflict", "Handle already taken");
    throw error;
  }
  if ((results[0]?.meta.changes ?? 0) === 0) return invalidInvite(c);

  c.header("Set-Cookie", session.cookie, { append: true });
  return respond(c, RegisterVerifyResponse, {
    user: { id: userId, handle, displayName, role: await roleOf(c, userId) },
    recoveryCodes: recovery.codes,
  });
};

async function roleOf(c: AppContext, userId: string): Promise<"user" | "admin"> {
  const row = await c.env.DB.prepare(USER_ROLE_SQL).bind(userId).first<{ role: "user" | "admin" }>();
  return row?.role ?? "user";
}
