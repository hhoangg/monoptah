// `POST /api/recovery-codes/regenerate` (A-05, fresh) and the "I saved these" ack. Regeneration is ONE batch that
// inserts the 10 new codes (batch = old max + 1), deletes every older code of the user, clears
// `users.recovery_ack_at` (the next sign-in forces the user to confirm they saved the new codes) and audits.
// Every statement after the first insert keys on that first row, so a parallel regenerate that lost the race
// writes nothing and answers 409 instead of leaving two live batches.
import type { Handler } from "hono";
import { RegenerateRecoveryResponse } from "@monoptah/protocol";
import type { Env } from "../../env.ts";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { auditStatement } from "../audit/audit.ts";
import { generateRecoveryCodes } from "./codes.ts";

export const MAX_BATCH_SQL = "SELECT coalesce(max(batch), 0) AS batch FROM recovery_codes WHERE user_id = ?";
/** First new code: only while no code newer than the batch we read exists. */
export const INSERT_FIRST_CODE_SQL =
  "INSERT INTO recovery_codes (id, user_id, code_hash, batch, created_at) SELECT ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM recovery_codes WHERE user_id = ? AND batch > ?)";
/** The other new codes: only while the first one exists. */
export const INSERT_NEXT_CODE_SQL =
  "INSERT INTO recovery_codes (id, user_id, code_hash, batch, created_at) SELECT ?, ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM recovery_codes WHERE id = ?)";
export const DELETE_OLD_CODES_SQL =
  "DELETE FROM recovery_codes WHERE user_id = ? AND batch <= ? AND EXISTS (SELECT 1 FROM recovery_codes WHERE id = ?)";
export const CLEAR_ACK_SQL = "UPDATE users SET recovery_ack_at = NULL WHERE id = ? AND EXISTS (SELECT 1 FROM recovery_codes WHERE id = ?)";
export const SET_ACK_SQL = "UPDATE users SET recovery_ack_at = ? WHERE id = ?";

export const regenerateRecoveryCodes: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const old = await c.env.DB.prepare(MAX_BATCH_SQL).bind(user.id).first<{ batch: number }>();
  const oldBatch = old?.batch ?? 0;
  const batch = oldBatch + 1;
  const now = nowSeconds();
  const fresh = await generateRecoveryCodes(c.env);
  const ids = fresh.hashes.map(() => crypto.randomUUID());
  const firstId = ids[0] ?? "";

  const results = await c.env.DB.batch([
    c.env.DB.prepare(INSERT_FIRST_CODE_SQL).bind(firstId, user.id, fresh.hashes[0], batch, now, user.id, oldBatch),
    ...fresh.hashes.slice(1).map((hash, i) => c.env.DB.prepare(INSERT_NEXT_CODE_SQL).bind(ids[i + 1], user.id, hash, batch, now, firstId)),
    c.env.DB.prepare(DELETE_OLD_CODES_SQL).bind(user.id, oldBatch, firstId),
    c.env.DB.prepare(CLEAR_ACK_SQL).bind(user.id, firstId),
    auditStatement(
      c.env,
      { action: "recovery.regenerate", meta: { batch }, actorUserId: user.id, targetType: "user", targetId: user.id, ts: now },
      { sql: "EXISTS (SELECT 1 FROM recovery_codes WHERE id = ?)", params: [firstId] },
    ),
  ]);
  if ((results[0]?.meta.changes ?? 0) === 0) return apiError(c, 409, "conflict", "Recovery codes were regenerated at the same time");
  return respond(c, RegenerateRecoveryResponse, { recoveryCodes: fresh.codes });
};

/**
 * "I saved these": sets `users.recovery_ack_at`. A null value at the next sign-in makes the client force a
 * regenerate. There is no protocol route for it yet (WS-K owns `api.ts`), so this is the handler logic only.
 */
export async function ackRecoveryCodes(env: Pick<Env, "DB">, userId: string, now: number = nowSeconds()): Promise<void> {
  await env.DB.prepare(SET_ACK_SQL).bind(now, userId).run();
}
