import { defineModule } from "../define.ts";
import { deleteDesktop, desktopMe, getAvatar, getDesktop, listDesktops, patchDesktop, unpairDesktop } from "./handlers.ts";

/** Routes of the `desktops` module (R-03, R-14a): the user's directory and the desktop's own `/api/desktop/me`. */
export const routes = defineModule("desktops", {
  "GET /api/desktops": listDesktops,
  "GET /api/desktops/:id": getDesktop,
  "PATCH /api/desktops/:id": patchDesktop,
  "GET /api/desktops/:id/avatar": getAvatar,
  "DELETE /api/desktops/:id": deleteDesktop,
  "GET /api/desktop/me": desktopMe,
  "DELETE /api/desktop/me": unpairDesktop,
});
