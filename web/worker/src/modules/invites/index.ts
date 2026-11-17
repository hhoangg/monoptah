import { defineModule } from "../define.ts";
import { createInvite, listInvites, revokeInvite } from "./handlers.ts";

/** Routes of the `invites` module: list, create (quota 3, gate) and revoke. */
export const routes = defineModule("invites", {
  "GET /api/invites": listInvites,
  "POST /api/invites": createInvite,
  "DELETE /api/invites/:id": revokeInvite,
});
