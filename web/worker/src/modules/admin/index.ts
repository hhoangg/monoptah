import { defineModule } from "../define.ts";
import { blockUser, getSettings, grantInvites, listAudit, listUsers, putSettings, unblockUser } from "./handlers.ts";

/** Routes of the `admin` module (A-09): users, block and unblock, invite grants, settings and the audit view. */
export const routes = defineModule("admin", {
  "GET /api/admin/users": listUsers,
  "POST /api/admin/users/:id/block": blockUser,
  "POST /api/admin/users/:id/unblock": unblockUser,
  "POST /api/admin/users/:id/invites": grantInvites,
  "GET /api/admin/settings": getSettings,
  "PUT /api/admin/settings": putSettings,
  "GET /api/admin/audit": listAudit,
});
