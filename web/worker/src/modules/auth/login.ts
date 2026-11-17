// Login (A-03): discoverable passkey sign-in. `login/options` has no allowCredentials, so the authenticator offers
// its resident credentials; `login/verify` finds the passkey by credential id, verifies the assertion and applies
// the counter rule (a refused counter is audited and no session is created).
import type { Handler } from "hono";
import { LoginOptionsResponse, LoginVerifyBody, LoginVerifyResponse } from "@monoptah/protocol";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { auditStatement } from "../audit/audit.ts";
import { consumeChallenge, storeChallenge } from "../passkeys/challenges.ts";
import { buildAuthenticationOptions, verifyAuthentication } from "../passkeys/webauthn.ts";
import { createSession } from "../sessions/session.ts";

export const PASSKEY_LOGIN_SQL =
  "SELECT p.id, p.user_id, p.public_key, p.counter, p.transports, u.handle, u.display_name, u.role, u.status FROM passkeys p JOIN users u ON u.id = p.user_id WHERE p.id = ?";
export const UPDATE_PASSKEY_USE_SQL = "UPDATE passkeys SET counter = ?, backed_up = ?, last_used_at = ? WHERE id = ?";

export interface PasskeyRow {
  id: string;
  user_id: string;
  public_key: ArrayBuffer | number[];
  counter: number;
  transports: string;
  handle: string;
  display_name: string;
  role: "user" | "admin";
  status: "active" | "blocked";
}

export function parseTransports(text: string): string[] {
  try {
    const value: unknown = JSON.parse(text);
    return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}

/** The stored key as bytes, whether the driver returns an ArrayBuffer or a number array. */
export function publicKeyBytes(row: Pick<PasskeyRow, "public_key">): Uint8Array {
  return new Uint8Array(row.public_key);
}

export function findPasskey(c: AppContext, credentialId: string): Promise<PasskeyRow | null> {
  return c.env.DB.prepare(PASSKEY_LOGIN_SQL).bind(credentialId).first<PasskeyRow>();
}

export const loginOptions: Handler<AppEnv> = async (c) => {
  const options = await buildAuthenticationOptions(c.env);
  const challengeId = await storeChallenge(c.env, { purpose: "login", challenge: options.challenge });
  return respond(c, LoginOptionsResponse, { challengeId, options: { ...options } });
};

const unauthorized = (c: AppContext): Response => apiError(c, 401, "unauthorized", "Sign-in failed");

export const loginVerify: Handler<AppEnv> = async (c) => {
  const body = LoginVerifyBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid sign-in request");

  const challenge = await consumeChallenge(c.env, { id: body.data.challengeId, purpose: "login" });
  if (!challenge) return apiError(c, 400, "invalid", "Challenge expired or already used");

  const passkey = await findPasskey(c, body.data.response.id);
  if (!passkey) return unauthorized(c);

  const result = await verifyAuthentication(
    c.env,
    body.data.response,
    challenge.challenge,
    { id: passkey.id, publicKey: publicKeyBytes(passkey), counter: passkey.counter, transports: parseTransports(passkey.transports) },
    passkey.user_id,
  );
  if (!result.ok && result.reason === "invalid") return unauthorized(c);

  const now = nowSeconds();
  const ip = await ipHash(c.env, clientIp(c.req.raw));
  if (!result.ok) {
    // Counter did not increase: the stored counter stays, the event is audited, no session.
    await auditStatement(c.env, {
      action: "account.login_refused_counter",
      meta: { passkeyId: passkey.id, ipHash: ip },
      actorKind: "user",
      actorUserId: passkey.user_id,
      targetType: "passkey",
      targetId: passkey.id,
      ts: now,
    }).run();
    return unauthorized(c);
  }
  if (passkey.status === "blocked") return apiError(c, 403, "blocked", "Account blocked");

  const session = await createSession(c.env, { userId: passkey.user_id, amr: "passkey", userAgent: c.req.header("User-Agent"), reauthAt: now, now });
  await c.env.DB.batch([
    c.env.DB.prepare(UPDATE_PASSKEY_USE_SQL).bind(result.newCounter, result.backedUp ? 1 : 0, now, passkey.id),
    session.statement,
    auditStatement(c.env, {
      action: "account.login",
      meta: { passkeyId: passkey.id, ipHash: ip },
      actorUserId: passkey.user_id,
      targetType: "user",
      targetId: passkey.user_id,
      ts: now,
    }),
  ]);
  c.header("Set-Cookie", session.cookie, { append: true });
  return respond(c, LoginVerifyResponse, { user: { id: passkey.user_id, handle: passkey.handle, displayName: passkey.display_name, role: passkey.role } });
};
