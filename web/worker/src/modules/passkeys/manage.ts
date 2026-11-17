// Passkey management (A-08): list, add, rename, delete. Add and the changes need a fresh session (a recovery_only
// session may add without a re-auth: it has no passkey to assert with, and stays recovery_only afterwards, so the
// client then signs in with the new passkey). Rename and delete answer 404 for a passkey of another user.
// "Never the last" is enforced in SQL: the DELETE only runs while the user has more than one passkey.
import type { Handler } from "hono";
import {
  AddPasskeyOptionsResponse,
  AddPasskeyVerifyBody,
  PasskeyResponse,
  PasskeysResponse,
  RenamePasskeyBody,
  type PasskeySchema,
} from "@monoptah/protocol";
import type { z } from "zod";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { auditStatement } from "../audit/audit.ts";
import { parseTransports } from "../auth/login.ts";
import { consumeChallenge, storeChallenge } from "./challenges.ts";
import { buildRegistrationOptions, verifyRegistration } from "./webauthn.ts";

type Passkey = z.output<typeof PasskeySchema>;

const PASSKEY_COLUMNS = "id, name, created_at, last_used_at, device_type, backed_up, transports";
export const LIST_PASSKEYS_SQL = `SELECT ${PASSKEY_COLUMNS} FROM passkeys WHERE user_id = ? ORDER BY created_at, id`;
export const GET_PASSKEY_SQL = `SELECT ${PASSKEY_COLUMNS} FROM passkeys WHERE id = ? AND user_id = ?`;
export const ADD_PASSKEY_EXCLUDE_SQL = "SELECT id, transports FROM passkeys WHERE user_id = ?";
/** A missing name becomes "Passkey N" with N = the user's passkeys after this one. */
export const INSERT_ADDED_PASSKEY_SQL = `INSERT INTO passkeys (id, user_id, public_key, counter, transports, device_type, backed_up, name, created_at)
  VALUES (?, ?, ?, ?, ?, ?, ?, coalesce(?, 'Passkey ' || ((SELECT count(*) FROM passkeys WHERE user_id = ?) + 1)), ?)`;
export const RENAME_PASSKEY_SQL = "UPDATE passkeys SET name = ? WHERE id = ? AND user_id = ?";
export const PASSKEY_EXISTS_SQL = "EXISTS (SELECT 1 FROM passkeys WHERE id = ? AND user_id = ?)";
export const COUNT_PASSKEYS_SQL = "SELECT count(*) AS n FROM passkeys WHERE user_id = ?";
/** The delete runs only for an own passkey while the user has another one. */
export const DELETE_PASSKEY_CONDITION_SQL = `${PASSKEY_EXISTS_SQL} AND (SELECT count(*) FROM passkeys WHERE user_id = ?) > 1`;
export const DELETE_PASSKEY_SQL = `DELETE FROM passkeys WHERE id = ? AND user_id = ? AND (SELECT count(*) FROM passkeys WHERE user_id = ?) > 1`;

interface PasskeyRow {
  id: string;
  name: string;
  created_at: number;
  last_used_at: number | null;
  device_type: string | null;
  backed_up: number;
  transports: string;
}

function toPasskey(row: PasskeyRow): Passkey {
  return {
    id: row.id,
    name: row.name,
    createdAt: row.created_at,
    lastUsedAt: row.last_used_at,
    deviceType: row.device_type,
    backedUp: row.backed_up === 1,
    transports: parseTransports(row.transports),
  };
}

const notFound = (c: AppContext): Response => apiError(c, 404, "not_found", "Passkey not found");

export const listPasskeys: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const { results } = await c.env.DB.prepare(LIST_PASSKEYS_SQL).bind(user.id).all<PasskeyRow>();
  return respond(c, PasskeysResponse, { passkeys: results.map(toPasskey) });
};

export const addPasskeyOptions: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const { results } = await c.env.DB.prepare(ADD_PASSKEY_EXCLUDE_SQL).bind(user.id).all<{ id: string; transports: string }>();
  const options = await buildRegistrationOptions(c.env, {
    userId: user.id,
    userName: user.handle,
    displayName: user.displayName,
    excludeCredentials: results.map((row) => ({ id: row.id, transports: parseTransports(row.transports) })),
  });
  const challengeId = await storeChallenge(c.env, { purpose: "addpasskey", challenge: options.challenge, userId: user.id });
  return respond(c, AddPasskeyOptionsResponse, { challengeId, options: { ...options } });
};

function isUniqueViolation(error: unknown): boolean {
  return error instanceof Error && /UNIQUE constraint failed/i.test(error.message);
}

export const addPasskeyVerify: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const body = AddPasskeyVerifyBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid passkey request");

  const challenge = await consumeChallenge(c.env, { id: body.data.challengeId, purpose: "addpasskey", userId: user.id });
  if (!challenge) return apiError(c, 400, "invalid", "Challenge expired or already used");
  const credential = await verifyRegistration(c.env, body.data.response, challenge.challenge);
  if (!credential) return apiError(c, 400, "invalid", "Passkey could not be verified");

  const now = nowSeconds();
  try {
    await c.env.DB.batch([
      c.env.DB.prepare(INSERT_ADDED_PASSKEY_SQL).bind(
        credential.credentialId,
        user.id,
        credential.publicKey.slice().buffer,
        credential.counter,
        JSON.stringify(credential.transports),
        credential.deviceType,
        credential.backedUp ? 1 : 0,
        body.data.name ?? null,
        user.id,
        now,
      ),
      auditStatement(c.env, { action: "passkey.add", meta: { passkeyId: credential.credentialId }, actorUserId: user.id, targetType: "passkey", targetId: credential.credentialId, ts: now }),
    ]);
  } catch (error) {
    // The credential id is the primary key: this authenticator is already registered (here or for another user).
    if (isUniqueViolation(error)) return apiError(c, 409, "conflict", "This passkey is already registered");
    throw error;
  }
  const row = await c.env.DB.prepare(GET_PASSKEY_SQL).bind(credential.credentialId, user.id).first<PasskeyRow>();
  if (!row) throw new Error("Added passkey not found");
  return respond(c, PasskeyResponse, { passkey: toPasskey(row) });
};

export const renamePasskey: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const body = RenamePasskeyBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid passkey name");
  const results = await c.env.DB.batch([
    c.env.DB.prepare(RENAME_PASSKEY_SQL).bind(body.data.name, id, user.id),
    auditStatement(
      c.env,
      { action: "passkey.rename", meta: { passkeyId: id }, actorUserId: user.id, targetType: "passkey", targetId: id },
      { sql: PASSKEY_EXISTS_SQL, params: [id, user.id] },
    ),
  ]);
  if ((results[0]?.meta.changes ?? 0) === 0) return notFound(c);
  const row = await c.env.DB.prepare(GET_PASSKEY_SQL).bind(id, user.id).first<PasskeyRow>();
  if (!row) return notFound(c);
  return respond(c, PasskeyResponse, { passkey: toPasskey(row) });
};

const lastPasskey = (c: AppContext): Response => apiError(c, 409, "last_passkey", "You cannot delete your last passkey");

export const deletePasskey: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const own = await c.env.DB.prepare(GET_PASSKEY_SQL).bind(id, user.id).first<PasskeyRow>();
  if (!own) return notFound(c);
  const count = (await c.env.DB.prepare(COUNT_PASSKEYS_SQL).bind(user.id).first<{ n: number }>())?.n ?? 0;
  if (count <= 1) return lastPasskey(c);

  // The audit INSERT comes first, with the same condition as the DELETE: both see the same rows in the batch.
  const results = await c.env.DB.batch([
    auditStatement(
      c.env,
      { action: "passkey.delete", meta: { passkeyId: id, remaining: count - 1 }, actorUserId: user.id, targetType: "passkey", targetId: id },
      { sql: DELETE_PASSKEY_CONDITION_SQL, params: [id, user.id, user.id] },
    ),
    c.env.DB.prepare(DELETE_PASSKEY_SQL).bind(id, user.id, user.id),
  ]);
  // A parallel delete took the other passkey first: this one is now the last.
  if ((results[1]?.meta.changes ?? 0) === 0) return lastPasskey(c);
  return c.json({ ok: true as const });
};
