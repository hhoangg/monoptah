import { defineModule } from "../define.ts";
import { logout } from "../sessions/handlers.ts";
import { loginOptions, loginVerify } from "./login.ts";
import { reauthOptions, reauthVerify } from "./reauth.ts";
import { registerOptions, registerVerify } from "./register.ts";

/** Routes of the `auth` module: passkey registration, login and re-auth (A-03), logout (A-04). `recover` is A-05. */
export const routes = defineModule("auth", {
  "POST /api/auth/register/options": registerOptions,
  "POST /api/auth/register/verify": registerVerify,
  "POST /api/auth/login/options": loginOptions,
  "POST /api/auth/login/verify": loginVerify,
  "POST /api/auth/reauth/options": reauthOptions,
  "POST /api/auth/reauth/verify": reauthVerify,
  "POST /api/auth/logout": logout,
});
