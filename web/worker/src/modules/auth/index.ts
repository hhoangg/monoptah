import { defineModule } from "../define.ts";
import { logout } from "../sessions/handlers.ts";

/** Routes of the `auth` module. `POST /api/auth/logout` is A-04; the passkey routes answer 501 until A-03. */
export const routes = defineModule("auth", {
  "POST /api/auth/logout": logout,
});
