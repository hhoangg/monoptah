// The three push listeners of the service worker (P-04), written against small structural types so they
// run under the WebWorker lib (sw.ts) and under vitest with a fake scope.
import { notificationFor, parsePushPayload, readPushData } from "./payload";
import { clickTarget } from "./safeUrl";

/** Posted to a window that is already open; features/push/navigationMessage.ts routes it. */
export const NAVIGATE_MESSAGE_TYPE = "monoptah:navigate";

export interface ClientLike {
  url: string;
  focus(): Promise<unknown>;
  postMessage(message: unknown): void;
}

export interface PushDeps {
  /** `self.location.origin`. */
  origin: string;
  showNotification(title: string, options: NotificationOptions): Promise<void>;
  /** Windows of this origin, controlled or not. */
  listWindowClients(): Promise<readonly ClientLike[]>;
  openWindow(url: string): Promise<unknown>;
  resubscribe(): Promise<unknown>;
}

interface Waitable {
  waitUntil(promise: Promise<unknown>): void;
}
export interface PushEventLike extends Waitable {
  data: { json(): unknown } | null;
}
export interface NotificationClickEventLike extends Waitable {
  notification: { data?: unknown; close(): void };
}
export type SubscriptionChangeEventLike = Waitable;

export function createPushHandlers(deps: PushDeps) {
  return {
    // Always shows something: iOS revokes a subscription whose pushes show no notification, so there is no
    // silent path, no foreground suppression and no visibility check (features.md 2.17).
    push(event: PushEventLike): void {
      const spec = notificationFor(parsePushPayload(readPushData(event.data)));
      event.waitUntil(deps.showNotification(spec.title, spec.options));
    },

    notificationclick(event: NotificationClickEventLike): void {
      event.notification.close();
      const data: unknown = event.notification.data;
      const url = clickTarget(typeof data === "object" && data !== null ? (data as { url?: unknown }).url : undefined);
      event.waitUntil(openOrFocus(deps, url));
    },

    pushsubscriptionchange(event: SubscriptionChangeEventLike): void {
      // A failed re-subscribe is retried by the page on its next open (features/push/subscribe.ts).
      event.waitUntil(deps.resubscribe().catch(() => undefined));
    },
  };
}

async function openOrFocus(deps: PushDeps, url: string): Promise<void> {
  const windows = await deps.listWindowClients();
  const existing = windows.find((client) => sameOrigin(client.url, deps.origin));
  if (!existing) {
    await deps.openWindow(url);
    return;
  }
  // The page routes the URL itself: no reload, the link stays up and the session resyncs (F-83).
  existing.postMessage({ type: NAVIGATE_MESSAGE_TYPE, url });
  await existing.focus();
}

function sameOrigin(raw: string, origin: string): boolean {
  try {
    return new URL(raw).origin === origin;
  } catch {
    return false;
  }
}
