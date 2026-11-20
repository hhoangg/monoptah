// The three push listeners of the service worker (P-04), written against small structural types so they
// run under the WebWorker lib (sw.ts) and under vitest with a fake scope.
import { notificationFor, parsePushPayload, readPushData } from "./payload";
import { FALLBACK_URL, clickTarget } from "./safeUrl";

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
  /** Leaves the target for a page that is not ready to hear a message (sw/pendingTarget.ts). Optional: omitted in older tests. */
  savePending?(url: string, at: number): Promise<unknown>;
  /** Milliseconds since the epoch. */
  now?(): number;
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
      event.waitUntil(handleClick(deps, url));
    },

    pushsubscriptionchange(event: SubscriptionChangeEventLike): void {
      // A failed re-subscribe is retried by the page on its next open (features/push/subscribe.ts).
      event.waitUntil(deps.resubscribe().catch(() => undefined));
    },
  };
}

async function handleClick(deps: PushDeps, url: string): Promise<void> {
  // Saved first and always: on iOS standalone, `openWindow` may open the app at its start URL and a window may be
  // suspended or still booting when the message arrives. The page takes the target once (features/push/register.ts).
  // `at` identifies this tap: the saved target and the message carry it, so the page routes a tap once.
  const at = (deps.now ?? Date.now)();
  if (deps.savePending && url !== FALLBACK_URL) {
    await Promise.resolve(deps.savePending(url, at)).catch(() => undefined);
  }
  await openOrFocus(deps, url, at);
}

async function openOrFocus(deps: PushDeps, url: string, at: number): Promise<void> {
  const windows = await deps.listWindowClients();
  const existing = windows.find((client) => sameOrigin(client.url, deps.origin));
  if (!existing) {
    // iOS can refuse (reject) this in a standalone app: the saved target still opens on the next start.
    await Promise.resolve(deps.openWindow(url)).catch(() => undefined);
    return;
  }
  // The page routes the URL itself: no reload, the link stays up and the session resyncs (F-83).
  try {
    existing.postMessage({ type: NAVIGATE_MESSAGE_TYPE, url, at });
  } catch {
    // The saved target covers it.
  }
  await Promise.resolve(existing.focus()).catch(() => undefined);
}

function sameOrigin(raw: string, origin: string): boolean {
  try {
    return new URL(raw).origin === origin;
  } catch {
    return false;
  }
}
