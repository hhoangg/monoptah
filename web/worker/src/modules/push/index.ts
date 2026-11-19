import { defineModule } from "../define.ts";
import { notify } from "./notify.ts";
import { pushTest } from "./test.ts";
import { getPrefs, getVapidKey, putPrefs, subscribe, unsubscribe } from "./subscriptions.ts";

/** Routes of the `push` module: VAPID key, subscriptions, prefs (P-01) and the notify sender (P-02). */
export const routes = defineModule("push", {
  "GET /api/push/vapid-key": getVapidKey,
  "POST /api/push/subscriptions": subscribe,
  "DELETE /api/push/subscriptions/:id": unsubscribe,
  "GET /api/push/prefs/:desktopId": getPrefs,
  "PUT /api/push/prefs/:desktopId": putPrefs,
  "POST /api/push/test": pushTest,
  "POST /api/push/notify": notify,
});
