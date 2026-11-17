import { defineModule } from "../define.ts";
import { listOwnAudit } from "./handlers.ts";

export { AUDIT_ACTIONS, AUDIT_ACTION_NAMES, FORBIDDEN_META_KEYS, type AuditAction, type AuditMeta } from "./actions.ts";
export { auditStatement, writeAudit, type AuditEvent } from "./audit.ts";

/** Routes of the `audit` module: `GET /api/audit` (own events, paged). */
export const routes = defineModule("audit", {
  "GET /api/audit": listOwnAudit,
});
