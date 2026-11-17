// Audit log writer (security.md 10). Callers either put `auditStatement(...)` into their own
// `env.DB.batch([...])` (so the event commits with the change it records) or call `writeAudit(...)`.
// Meta is validated against the strict schema of the action (actions.ts); invalid meta throws.
import type { Env } from "../../env.ts";
import { nowSeconds } from "../../lib/time.ts";
import { AUDIT_ACTIONS, type AuditAction, type AuditActorKind, type AuditMeta } from "./actions.ts";

/** The input of one audit event. `meta` is typed per action; the schema also checks it at run time. */
export type AuditEvent = {
  [A in AuditAction]: {
    action: A;
    meta: AuditMeta<A>;
    /** Defaults to the first actor kind of the action (see actions.ts). */
    actorKind?: AuditActorKind;
    /** The acting user id; null or absent for `system` events and anonymous ones. */
    actorUserId?: string | null;
    targetType?: string;
    targetId?: string;
    /** Unix seconds; defaults to now. */
    ts?: number;
  };
}[AuditAction];

export const INSERT_AUDIT_SQL =
  "INSERT INTO audit_log (id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

/** A SQL condition (and its bound values) that makes an INSERT write its row only when it holds. */
export interface StatementCondition {
  sql: string;
  params: unknown[];
}

const CONDITIONAL_INSERT_AUDIT_SQL = (condition: string): string =>
  `INSERT INTO audit_log (id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta) SELECT ?, ?, ?, ?, ?, ?, ?, ? WHERE ${condition}`;

/**
 * Builds the INSERT of one audit event, for use in a `D1Database.batch`. With `when`, the row is written only
 * if the condition holds (`INSERT ... SELECT ... WHERE <when>`), so a batch can drop its audit rows when its
 * first statement changed nothing. Throws on an unknown action, an
 * actor kind the action does not allow, or meta that does not match the action's strict schema.
 */
export function auditStatement(env: Pick<Env, "DB">, event: AuditEvent, when?: StatementCondition): D1PreparedStatement {
  const values = auditValues(event);
  if (!when) return env.DB.prepare(INSERT_AUDIT_SQL).bind(...values);
  return env.DB.prepare(CONDITIONAL_INSERT_AUDIT_SQL(when.sql)).bind(...values, ...when.params);
}

/** Validates the event and returns the eight column values of the audit INSERT. */
function auditValues(event: AuditEvent): unknown[] {
  const entry = AUDIT_ACTIONS[event.action];
  if (!entry) throw new Error(`Unknown audit action: ${String(event.action)}`);
  const actorKind = event.actorKind ?? entry.actorKinds[0];
  if (!entry.actorKinds.includes(actorKind)) throw new Error(`Audit action ${event.action} does not allow actor kind ${actorKind}`);
  const parsed = entry.meta.safeParse(event.meta);
  if (!parsed.success) {
    // Issue paths and codes only: the message of a failed parse may echo the offending value.
    const paths = parsed.error.issues.map((issue) => `${issue.code}:${issue.path.join(".")}`).join(", ");
    throw new Error(`Invalid meta for audit action ${event.action}: ${paths}`);
  }
  return [
    crypto.randomUUID(),
    event.ts ?? nowSeconds(),
    event.actorUserId ?? null,
    actorKind,
    event.action,
    event.targetType ?? null,
    event.targetId ?? null,
    JSON.stringify(parsed.data),
  ];
}

/**
 * Condition for an audit INSERT that directly follows the write it records in the same batch: true only when that
 * write changed a row (`changes()` is the count of the previous statement). A lost race (the parallel call wrote
 * first, so this write changed nothing) then leaves no audit row.
 */
export const PREVIOUS_CHANGED: StatementCondition = { sql: "changes() > 0", params: [] };

const COUNTED_INSERT_AUDIT_SQL =
  "INSERT INTO audit_log (id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta) SELECT ?, ?, ?, ?, ?, ?, ?, json_set(?, '$.count', changes()) WHERE changes() > 0";

/**
 * Like `auditStatement` with `PREVIOUS_CHANGED`, and `meta.count` is the number of rows the previous statement
 * changed, computed in the batch instead of read before it. The event needs a `count` in its meta; the value given
 * is a placeholder that only the schema check sees.
 */
export function auditStatementCounted(env: Pick<Env, "DB">, event: Extract<AuditEvent, { meta: { count: number } }>): D1PreparedStatement {
  return env.DB.prepare(COUNTED_INSERT_AUDIT_SQL).bind(...auditValues(event));
}

/** Writes one audit event now. Prefer `auditStatement` inside the batch of the change being recorded. */
export async function writeAudit(env: Pick<Env, "DB">, event: AuditEvent): Promise<void> {
  await auditStatement(env, event).run();
}
