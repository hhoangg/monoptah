// User-facing text of the admin screens.
import { isApiError } from "@/features/accounts/api";
import { settingsErrorMessage } from "@/features/settings/copy";
import type { AdminAuditEvent, AdminUser } from "./adminApi";
import { handleOf } from "./tree";

export const NOT_AVAILABLE = "This page is not available for your account.";

/** One line for a failed admin call. `conflict` has a different meaning per action, so callers pass their own. */
export function adminErrorMessage(error: unknown, conflict?: string): string {
  if (isApiError(error, "forbidden")) return NOT_AVAILABLE;
  if (isApiError(error, "not_found")) return "That user no longer exists. The list was refreshed.";
  if (isApiError(error, "conflict") && conflict) return conflict;
  if (isApiError(error, "invalid")) return "The server did not accept that request. Check the values and try again.";
  return settingsErrorMessage(error);
}

export const BLOCK_CONFLICT = "You cannot block yourself.";
export const UNBLOCK_CONFLICT = "This user is not blocked any more. The list was refreshed.";
export const GRANT_CONFLICT = "A user can hold at most 1000 invites in total.";

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** A short title for an audit event; unknown actions show their key. */
export function auditTitle(event: AdminAuditEvent): string {
  const meta = event.meta;
  switch (event.action) {
    case "user.block": {
      const count = typeof meta.count === "number" ? meta.count : null;
      if (meta.cascade === true) return count === null ? "Blocked a user and everyone they invited" : `Blocked a branch (${plural(count, "user", "users")})`;
      return "Blocked a user";
    }
    case "user.unblock":
      return "Unblocked a user";
    case "admin.invites_grant":
      return typeof meta.count === "number" ? `Granted ${plural(meta.count, "invite", "invites")}` : "Granted invites";
    case "admin.settings_change":
      return meta.invitesPaused === true ? "Paused invites" : meta.invitesPaused === false ? "Resumed invites" : "Changed settings";
    case "user.register":
      return "Account created";
    case "invite.create":
      return "Invite created";
    case "invite.use":
      return "Invite used";
    case "invite.revoke":
      return "Invite revoked";
    case "passkey.add":
      return "Passkey added";
    case "recovery.regenerate":
      return "Recovery codes regenerated";
    case "pairing.approve":
      return "Desktop paired";
    case "login":
      return "Signed in";
    default:
      return event.action;
  }
}

/** "by @ada on @ben": the actor and, when there is one, the user the event is about. */
export function auditDetail(event: AdminAuditEvent, users: readonly AdminUser[] | undefined): string {
  const actor = event.actorKind === "system" ? "System" : event.actorKind === "desktop" ? "A desktop" : ((users && handleOf(users, event.actorUserId)) ?? "Unknown user");
  const target = event.targetType === "user" && users ? handleOf(users, event.targetId) : null;
  return target && target !== actor ? `By ${actor}, about ${target}` : `By ${actor}`;
}
