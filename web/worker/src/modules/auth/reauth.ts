// Re-auth (A-03): a fresh passkey assertion by the session's own user sets `sessions.reauth_at`, which opens the
// 5 minute "fresh" window of security.md 3.3. A recovery-only session never gets here (the resolver answers 403
// `recovery_only`: it has no passkey to assert with).
import type { Handler } from "hono";
import { ReauthOptionsResponse, ReauthVerifyBody, ReauthVerifyResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { consumeChallenge, storeChallenge } from "../passkeys/challenges.ts";
import { buildAuthenticationOptions, verifyAuthentication } from "../passkeys/webauthn.ts";
import { auditCounterRefused, findPasskey, parseTransports, PASSKEY_USE_APPLIED_SQL, passkeyUseParams, publicKeyBytes, UPDATE_PASSKEY_USE_SQL } from "./login.ts";

export const USER_PASSKEYS_SQL = "SELECT id, transports FROM passkeys WHERE user_id = ?";
/** Runs right after `UPDATE_PASSKEY_USE_SQL` in the batch and only when that update changed a row. */
export const SET_REAUTH_SQL = `UPDATE sessions SET reauth_at = ? WHERE id_hash = ? AND ${PASSKEY_USE_APPLIED_SQL}`;

export const reauthOptions: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const { results } = await c.env.DB.prepare(USER_PASSKEYS_SQL).bind(user.id).all<{ id: string; transports: string }>();
  const options = await buildAuthenticationOptions(c.env, results.map((row) => ({ id: row.id, transports: parseTransports(row.transports) })));
  const challengeId = await storeChallenge(c.env, { purpose: "reauth", challenge: options.challenge, userId: user.id });
  return respond(c, ReauthOptionsResponse, { challengeId, options: { ...options } });
};

export const reauthVerify: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  const body = ReauthVerifyBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid re-authentication request");

  const challenge = await consumeChallenge(c.env, { id: body.data.challengeId, purpose: "reauth", userId: user.id });
  if (!challenge) return apiError(c, 400, "invalid", "Challenge expired or already used");

  // 403, not 401: a 401 from a session route tells the client its session is gone.
  const failed = apiError(c, 403, "forbidden", "Re-authentication failed");
  const passkey = await findPasskey(c, body.data.response.id);
  if (!passkey || passkey.user_id !== user.id) return failed;

  const result = await verifyAuthentication(
    c.env,
    body.data.response,
    challenge.challenge,
    { id: passkey.id, publicKey: publicKeyBytes(passkey), counter: passkey.counter, transports: parseTransports(passkey.transports) },
    user.id,
  );
  const now = nowSeconds();
  if (!result.ok) {
    if (result.reason === "counter") await auditCounterRefused(c, { passkeyId: passkey.id, userId: user.id, ipHash: await ipHash(c.env, clientIp(c.req.raw)), now });
    return failed;
  }

  // The session is marked fresh only when the counter update changed a row (a parallel assertion with the same
  // counter would not), otherwise it is answered like a counter decrease.
  const results = await c.env.DB.batch([
    c.env.DB.prepare(UPDATE_PASSKEY_USE_SQL).bind(...passkeyUseParams(result.newCounter, result.backedUp, now, passkey.id)),
    c.env.DB.prepare(SET_REAUTH_SQL).bind(now, session.idHash),
  ]);
  if ((results[0]?.meta.changes ?? 0) === 0) {
    await auditCounterRefused(c, { passkeyId: passkey.id, userId: user.id, ipHash: await ipHash(c.env, clientIp(c.req.raw)), now });
    return failed;
  }
  return respond(c, ReauthVerifyResponse, { ok: true, reauthAt: now });
};
