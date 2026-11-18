// The page side of a notification click (P-04): when a window is already open the worker posts
// `{type: "monoptah:navigate", url}` instead of reloading it, and the router goes there, which also
// resyncs the session (F-83). The URL is validated again here: a message is not trusted just because it
// arrived on the service worker channel.
import { NAVIGATE_MESSAGE_TYPE } from "@/sw/pushHandlers";
import { FALLBACK_URL, safeNotificationUrl } from "@/sw/safeUrl";

/** The safe URL carried by a navigate message, or null for any other message. */
export function navigationTarget(data: unknown): string | null {
  if (typeof data !== "object" || data === null) return null;
  const message = data as { type?: unknown; url?: unknown };
  if (message.type !== NAVIGATE_MESSAGE_TYPE) return null;
  // The root is what a click on a generic notice sends.
  return message.url === FALLBACK_URL ? FALLBACK_URL : safeNotificationUrl(message.url);
}

type MessageSource = Pick<EventTarget, "addEventListener" | "removeEventListener">;

/** Routes navigate messages from the service worker. Returns the function that stops listening. */
export function installNavigationListener(navigate: (url: string) => void, source?: MessageSource): () => void {
  const target = source ?? (typeof navigator !== "undefined" && "serviceWorker" in navigator ? navigator.serviceWorker : undefined);
  if (!target) return () => undefined;
  const listener = (event: Event) => {
    const url = navigationTarget((event as MessageEvent).data);
    if (url) navigate(url);
  };
  target.addEventListener("message", listener);
  return () => target.removeEventListener("message", listener);
}
