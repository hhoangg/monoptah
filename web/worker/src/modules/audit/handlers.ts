import type { Handler } from "hono";
import { AuditResponse, PageQuery } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { toBase64Url } from "../../lib/crypto.ts";
import { requireAuth } from "../../middleware/auth.ts";

export const DEFAULT_PAGE_SIZE = 50;

/** First page of one actor's events, newest first. Uses `audit_log_actor_user_id_ts_id_idx` (migration 0002). */
export const AUDIT_PAGE_SQL =
  "SELECT id, ts, action, target_type, target_id, meta FROM audit_log WHERE actor_user_id = ? ORDER BY ts DESC, id DESC LIMIT ?";
/** Following pages: rows strictly before the cursor `(ts, id)` in the same order. */
export const AUDIT_PAGE_AFTER_SQL =
  "SELECT id, ts, action, target_type, target_id, meta FROM audit_log WHERE actor_user_id = ? AND (ts, id) < (?, ?) ORDER BY ts DESC, id DESC LIMIT ?";

interface AuditRow {
  id: string;
  ts: number;
  action: string;
  target_type: string | null;
  target_id: string | null;
  meta: string;
}

/** Opaque cursor of the last row of a page: base64url of `ts:id`. */
export function encodeCursor(ts: number, id: string): string {
  return toBase64Url(new TextEncoder().encode(`${ts}:${id}`));
}

/** Parses a cursor from `encodeCursor`; null when it is not one. */
export function decodeCursor(cursor: string): { ts: number; id: string } | null {
  if (!/^[A-Za-z0-9_-]+$/.test(cursor)) return null;
  try {
    const text = atob(cursor.replaceAll("-", "+").replaceAll("_", "/"));
    const match = /^(\d{1,15}):([A-Za-z0-9_.-]{1,128})$/.exec(text);
    return match ? { ts: Number(match[1]), id: match[2] as string } : null;
  } catch {
    return null;
  }
}

/** `GET /api/audit`: the caller's own events (actor = the user), newest first, paged by `(ts, id)` cursor. */
export const listOwnAudit: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const query = PageQuery.safeParse(c.req.query());
  if (!query.success) return apiError(c, 400, "invalid", "Invalid page query");
  const limit = query.data.limit ?? DEFAULT_PAGE_SIZE;
  let stmt: D1PreparedStatement;
  if (query.data.cursor === undefined) {
    stmt = c.env.DB.prepare(AUDIT_PAGE_SQL).bind(user.id, limit + 1);
  } else {
    const cursor = decodeCursor(query.data.cursor);
    if (!cursor) return apiError(c, 400, "invalid", "Invalid cursor");
    stmt = c.env.DB.prepare(AUDIT_PAGE_AFTER_SQL).bind(user.id, cursor.ts, cursor.id, limit + 1);
  }
  const { results } = await stmt.all<AuditRow>();
  const page = results.slice(0, limit);
  const last = page[page.length - 1];
  return respond(c, AuditResponse, {
    events: page.map((row) => ({
      id: row.id,
      ts: row.ts,
      action: row.action,
      targetType: row.target_type,
      targetId: row.target_id,
      meta: JSON.parse(row.meta) as Record<string, unknown>,
    })),
    nextCursor: results.length > limit && last ? encodeCursor(last.ts, last.id) : null,
  });
};
