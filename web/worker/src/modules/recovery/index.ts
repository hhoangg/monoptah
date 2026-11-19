import { defineModule } from "../define.ts";
import { ackRecoveryCodes, regenerateRecoveryCodes } from "./regenerate.ts";

/** Routes of the `recovery` module in the protocol registry. `POST /api/auth/recover` lives in the `auth` module. */
export const routes = defineModule("recovery", {
  "POST /api/recovery-codes/regenerate": regenerateRecoveryCodes,
  "POST /api/recovery-codes/ack": ackRecoveryCodes,
});
