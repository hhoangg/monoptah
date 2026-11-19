import { defineModule } from "../define.ts";
import { addPasskeyOptions, addPasskeyVerify, deletePasskey, listPasskeys, renamePasskey } from "./manage.ts";

/** Routes of the `passkeys` module in the protocol registry (A-08). */
export const routes = defineModule("passkeys", {
  "GET /api/passkeys": listPasskeys,
  "POST /api/passkeys/options": addPasskeyOptions,
  "POST /api/passkeys/verify": addPasskeyVerify,
  "PATCH /api/passkeys/:id": renamePasskey,
  "DELETE /api/passkeys/:id": deletePasskey,
});
