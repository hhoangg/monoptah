import { defineModule } from "../define.ts";
import { getMe, patchMe } from "./handlers.ts";

/** Routes of the `me` module. `DELETE /api/me` (account deletion) is ticket U-11 and answers 501 until then. */
export const routes = defineModule("me", {
  "GET /api/me": getMe,
  "PATCH /api/me": patchMe,
});
