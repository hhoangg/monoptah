import { defineModule } from "../define.ts";
import { consumeSelectionTicket, consumeTerminalTicket, createSelectionTicket, createTerminalTicket, desktopGrantState, desktopGrants, listGrants, putGrant, requestGrant } from "./handlers.ts";

/** Routes of the `grants` module (R-04, R-15): the web side and the desktop token side of the grant state machine. */
export const routes = defineModule("grants", {
  "POST /api/desktops/:id/grants": requestGrant,
  "GET /api/desktops/:id/grants": listGrants,
  "PUT /api/desktops/:id/grants/:deviceId": putGrant,
  "POST /api/desktops/:id/grants/:deviceId/selection-ticket": createSelectionTicket,
  "POST /api/desktops/:id/grants/:deviceId/terminal-ticket": createTerminalTicket,
  "GET /api/desktop/grants": desktopGrants,
  "POST /api/desktop/grants/:deviceId/selection-ticket": consumeSelectionTicket,
  "POST /api/desktop/grants/:deviceId/terminal-ticket": consumeTerminalTicket,
  "POST /api/desktop/grants/:deviceId/state": desktopGrantState,
});
