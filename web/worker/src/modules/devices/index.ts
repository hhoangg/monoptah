import { defineModule } from "../define.ts";
import { forgetDevice, listDevices, patchDevice, registerDevice } from "./handlers.ts";

/** Routes of the `devices` module (R-04): register, list, rename, forget. */
export const routes = defineModule("devices", {
  "POST /api/devices": registerDevice,
  "GET /api/devices": listDevices,
  "PATCH /api/devices/:id": patchDevice,
  "DELETE /api/devices/:id": forgetDevice,
});
