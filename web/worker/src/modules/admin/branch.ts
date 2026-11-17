// The branch of the invite tree that an admin block touches (A-09, security.md 4). Every statement of the block batch
// repeats the same recursive CTE over `users.invited_by`, because D1 has no temporary tables across the statements
// of a batch. The CTE reads only `invited_by`, which the batch never changes, so each statement sees the same
// members. Bind order of every statement: the root user id (inside the CTE), then the statement's own values, then
// the acting admin id (inside `MEMBERS`, which excludes the admin).
import { MAX_REVOKE_CALLS } from "../sessions/session.ts";

/**
 * `WITH` clause naming the branch: the root and, with `cascade`, every descendant (a user invited by a member, at any
 * depth). `UNION` (not `UNION ALL`) keeps the walk finite even for corrupt data with a cycle. Uses `users_invited_by_idx`.
 */
export function branchCte(cascade: boolean): string {
  return cascade
    ? "WITH RECURSIVE branch(id) AS (SELECT id FROM users WHERE id = ? UNION SELECT u.id FROM users u JOIN branch b ON u.invited_by = b.id)"
    : "WITH branch(id) AS (SELECT id FROM users WHERE id = ?)";
}

/** The members of the branch except the acting admin, who is never blocked. One `?`, the admin id. */
export const MEMBERS = "(SELECT id FROM branch WHERE id <> ?)";

/** Members that are active now: the users this block will change. Binds root, admin. */
export const countActiveSql = (cascade: boolean): string => `${branchCte(cascade)} SELECT count(*) AS n FROM users WHERE status = 'active' AND id IN ${MEMBERS}`;

/** Live desktops of the members, for `room.revoke`, at most `MAX_REVOKE_CALLS` (the subrequest cap). Binds root, admin. */
export const listDesktopsSql = (cascade: boolean): string =>
  `${branchCte(cascade)} SELECT id FROM desktops WHERE revoked_at IS NULL AND user_id IN ${MEMBERS} LIMIT ${MAX_REVOKE_CALLS}`;

/** Marks the active members blocked. A member that is already blocked keeps its own reason and `blocked_via`. Binds root, at, reason, via, admin. */
export const blockUsersSql = (cascade: boolean): string =>
  `${branchCte(cascade)} UPDATE users SET status = 'blocked', blocked_at = ?, blocked_reason = ?, blocked_via = ? WHERE status = 'active' AND id IN ${MEMBERS}`;

/** Binds root, admin. */
export const deleteSessionsSql = (cascade: boolean): string => `${branchCte(cascade)} DELETE FROM sessions WHERE user_id IN ${MEMBERS}`;

/** Revokes every live token of the members' desktops (the desktop row is kept). Binds root, at, admin. */
export const revokeTokensSql = (cascade: boolean): string =>
  `${branchCte(cascade)} UPDATE desktop_tokens SET revoked_at = ? WHERE revoked_at IS NULL AND desktop_id IN (SELECT id FROM desktops WHERE user_id IN ${MEMBERS})`;

/** Revokes the grants on the members' desktops (uses `device_grants_desktop_id_idx`). Binds root, at, admin. */
export const revokeDesktopGrantsSql = (cascade: boolean): string =>
  `${branchCte(cascade)} UPDATE device_grants SET state = 'revoked', updated_at = ? WHERE state <> 'revoked' AND desktop_id IN (SELECT id FROM desktops WHERE user_id IN ${MEMBERS})`;

/** Revokes the grants of the members' devices (uses the primary key `(device_id, desktop_id)`). Binds root, at, admin. */
export const revokeDeviceGrantsSql = (cascade: boolean): string =>
  `${branchCte(cascade)} UPDATE device_grants SET state = 'revoked', updated_at = ? WHERE state <> 'revoked' AND device_id IN (SELECT id FROM devices WHERE user_id IN ${MEMBERS})`;

/** Binds root, admin. */
export const deletePushSubscriptionsSql = (cascade: boolean): string => `${branchCte(cascade)} DELETE FROM push_subscriptions WHERE user_id IN ${MEMBERS}`;

/** Revokes the members' unused invites; used invites keep their row (the invitee exists). Binds root, at, admin. */
export const revokeInvitesSql = (cascade: boolean): string =>
  `${branchCte(cascade)} UPDATE invites SET revoked_at = ? WHERE used_at IS NULL AND revoked_at IS NULL AND created_by IN ${MEMBERS}`;
