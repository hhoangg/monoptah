// Re-auth (A-03): a fresh passkey assertion by the session's own user sets `sessions.reauth_at`, which opens the
// 5 minute "fresh" window of security.md 3.3. A recovery-only session never gets here (the resolver answers 403
// `recovery_only`: it has no passkey to assert with).
import type { Handler } from "hono";
import { ReauthOptionsResponse, ReauthVerifyBody, ReauthVerifyResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { auditStatement } from "../audit/audit.ts";
import { consumeChallenge, storeChallenge } from "../passkeys/challenges.ts";
import { buildAuthenticationOptions, verifyAuthentication } from "../passkeys/webauthn.ts";
import { findPasskey, parseTransports, publicKeyBytes, UPDATE_PASSKEY_USE_SQL } from "./login.ts";

export const USER_PASSKEYS_SQL = "SELECT id, transports FROM passkeys WHERE user_id = ?";
export const SET_REAUTH_SQL = "UPDATE sessions SET reauth_at = ? WHERE id_hash = ?";

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
    if (result.reason === "counter") {
      await auditStatement(c.env, {
        action: "account.login_refused_counter",
        meta: { passkeyId: passkey.id, ipHash: await ipHash(c.env, clientIp(c.req.raw)) },
        actorKind: "user",
        actorUserId: user.id,
        targetType: "passkey",
        targetId: passkey.id,
        ts: now,
      }).run();
    }
    return failed;
  }

  await c.env.DB.batch([
    c.env.DB.prepare(UPDATE_PASSKEY_USE_SQL).bind(result.newCounter, result.backedUp ? 1 : 0, now, passkey.id),
    c.env.DB.prepare(SET_REAUTH_SQL).bind(now, session.idHash),
  ]);
  return respond(c, ReauthVerifyResponse, { ok: true, reauthAt: now });
};
