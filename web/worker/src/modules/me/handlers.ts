import type { Handler } from "hono";
import { MeResponse, PatchMeBody, PatchMeResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { requireAuth } from "../../middleware/auth.ts";

/**
 * Invites the user may still create: `invites_granted` minus the invites they created that are not (revoked and
 * unused). An invite that was used stays counted; a revoked unused one is given back. Uses `invites_created_by_idx`.
 */
export const INVITES_USED_SQL = "SELECT count(*) AS n FROM invites WHERE created_by = ? AND NOT (revoked_at IS NOT NULL AND used_at IS NULL)";
export const ME_ROW_SQL = "SELECT invites_granted, recovery_ack_at FROM users WHERE id = ?";
export const PATCH_DISPLAY_NAME_SQL = "UPDATE users SET display_name = ? WHERE id = ?";

/** `GET /api/me`: the user, the invites left, whether the session is recovery-only, and the recovery ack. */
export const getMe: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  const [row, used] = await Promise.all([
    c.env.DB.prepare(ME_ROW_SQL).bind(user.id).first<{ invites_granted: number; recovery_ack_at: number | null }>(),
    c.env.DB.prepare(INVITES_USED_SQL).bind(user.id).first<{ n: number }>(),
  ]);
  if (!row) return apiError(c, 401, "unauthorized", "Sign in required");
  return respond(c, MeResponse, {
    user: { id: user.id, handle: user.handle, displayName: user.displayName, role: user.role },
    invitesLeft: Math.max(0, row.invites_granted - (used?.n ?? 0)),
    recoveryOnly: session.recoveryOnly,
    recoveryAckAt: row.recovery_ack_at,
  });
};

/**
 * `PATCH /api/me`: changes the display name. Not audited: security.md 10 lists no profile event, and the
 * display name is user content that `audit_log.meta` must not hold.
 */
export const patchMe: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const body = PatchMeBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid display name");
  await c.env.DB.prepare(PATCH_DISPLAY_NAME_SQL).bind(body.data.displayName, user.id).run();
  return respond(c, PatchMeResponse, { user: { id: user.id, handle: user.handle, displayName: body.data.displayName, role: user.role } });
};
