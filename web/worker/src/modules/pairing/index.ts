import { defineModule } from "../define.ts";
import { pairApprove, pairDeny, pairInfo, pairPoll, pairStart } from "./handlers.ts";

/** Routes of the `pairing` module (R-03): start, poll, info, approve, deny. */
export const routes = defineModule("pairing", {
  "POST /api/pair/start": pairStart,
  "POST /api/pair/poll": pairPoll,
  "GET /api/pair/:userCode": pairInfo,
  "POST /api/pair/:userCode/approve": pairApprove,
  "POST /api/pair/:userCode/deny": pairDeny,
});
