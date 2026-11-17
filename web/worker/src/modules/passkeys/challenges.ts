// WebAuthn challenges (`challenges` table, security.md 3.1): stored for 5 minutes, consumed once, bound to a
// purpose and, for re-auth and add-passkey, to a user. Consumption is one atomic `DELETE ... RETURNING`, so a
// second use of the same id finds nothing even when the first verification failed afterwards.
import type { Env } from "../../env.ts";
import { nowSeconds } from "../../lib/time.ts";

export type ChallengePurpose = "register" | "login" | "reauth" | "addpasskey";

export const CHALLENGE_TTL_SECONDS = 300;

export const INSERT_CHALLENGE_SQL = "INSERT INTO challenges (id, purpose, user_id, challenge, payload, expires_at) VALUES (?, ?, ?, ?, ?, ?)";
/** `user_id IS ?` binds NULL for register and login, and the user id for re-auth and add-passkey. */
export const CONSUME_CHALLENGE_SQL =
  "DELETE FROM challenges WHERE id = ? AND purpose = ? AND user_id IS ? AND expires_at > ? RETURNING challenge, payload";

export interface NewChallenge {
  purpose: ChallengePurpose;
  /** The base64url challenge of the WebAuthn options. */
  challenge: string;
  /** Required for `reauth` and `addpasskey`. */
  userId?: string;
  /** Serialized to JSON. Never put a secret in it (an invite code, for example, is stored hashed). */
  payload?: unknown;
  now?: number;
}

/** Stores a challenge and returns its id (the `challengeId` the client sends back). */
export async function storeChallenge(env: Pick<Env, "DB">, input: NewChallenge): Promise<string> {
  const id = crypto.randomUUID();
  const expiresAt = (input.now ?? nowSeconds()) + CHALLENGE_TTL_SECONDS;
  await env.DB.prepare(INSERT_CHALLENGE_SQL)
    .bind(id, input.purpose, input.userId ?? null, input.challenge, input.payload === undefined ? null : JSON.stringify(input.payload), expiresAt)
    .run();
  return id;
}

export interface ConsumedChallenge {
  challenge: string;
  payload: unknown;
}

/**
 * Deletes and returns the challenge, or null when it does not exist, was used, expired, belongs to another
 * purpose or (for the user bound purposes) to another user. A challenge that does not match is not consumed.
 */
export async function consumeChallenge(
  env: Pick<Env, "DB">,
  input: { id: string; purpose: ChallengePurpose; userId?: string; now?: number },
): Promise<ConsumedChallenge | null> {
  const row = await env.DB.prepare(CONSUME_CHALLENGE_SQL)
    .bind(input.id, input.purpose, input.userId ?? null, input.now ?? nowSeconds())
    .first<{ challenge: string; payload: string | null }>();
  if (!row) return null;
  return { challenge: row.challenge, payload: row.payload === null ? null : (JSON.parse(row.payload) as unknown) };
}
