// What the banners and the composer show about the connection (F-07, F-42, N-17).
// Browser online state first; for a desktop route the link state of src/link on top.
import { useSyncExternalStore } from "react";
import type { LinkState } from "@/link";

export type ConnectivityKind = "online" | "offline" | "desktop_offline" | "reconnecting";

export interface Connectivity {
  kind: ConnectivityKind;
  /** The sentence for a banner or a composer note; null when everything is fine. */
  label: string | null;
  /** Epoch ms the desktop was last heard from, when `kind` is `desktop_offline`. */
  lastSeenAt: number | null;
  /** Mutations are refused in every state but `online` (they are never queued, architecture.md 3.6). */
  blocksSending: boolean;
}

function subscribeOnline(listener: () => void): () => void {
  window.addEventListener("online", listener);
  window.addEventListener("offline", listener);
  return () => {
    window.removeEventListener("online", listener);
    window.removeEventListener("offline", listener);
  };
}

/** `navigator.onLine`, following the `online` and `offline` events. */
export function useOnline(): boolean {
  return useSyncExternalStore(subscribeOnline, () => navigator.onLine, () => true);
}

type LinkSlice = Pick<LinkState, "status" | "presence" | "blocked" | "lastFrameAt">;

/**
 * Pure mapping. `link` is null outside a desktop route. A blocked link (revoked, key changed, login
 * required) is not a connectivity problem: its own screen explains it, so the banner stays quiet.
 */
export function describeConnectivity(online: boolean, link: LinkSlice | null): Connectivity {
  if (!online) return { kind: "offline", label: "You are offline", lastSeenAt: null, blocksSending: true };
  if (link && link.blocked === null) {
    if (link.status === "reconnecting") return { kind: "reconnecting", label: "Reconnecting...", lastSeenAt: null, blocksSending: true };
    if (link.status === "ready" && link.presence === "offline") {
      return { kind: "desktop_offline", label: "Desktop offline", lastSeenAt: link.lastFrameAt, blocksSending: true };
    }
  }
  return { kind: "online", label: null, lastSeenAt: null, blocksSending: false };
}

/** "5 min ago" style text for the last-seen note; coarse on purpose. */
export function formatLastSeen(lastSeenAt: number, now: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - lastSeenAt) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}
