import { HealthResponse, PushSubscriptionBody, PushSubscriptionResponse, VapidKeyResponse } from "@monoptah/protocol";
import { endpoint } from "@/features/accounts/api";

export const pushEndpoints = {
  vapidKey: endpoint("GET", "/api/push/vapid-key", VapidKeyResponse),
  subscribe: endpoint("POST", "/api/push/subscriptions", PushSubscriptionResponse, PushSubscriptionBody),
  /** `{ ok: true }`, the same body every confirming route sends. */
  unsubscribe: endpoint("DELETE", "/api/push/subscriptions/:id", HealthResponse),
} as const;
