// Which blocking screen the link state calls for. Owner: WS-M (W-04). Pure, so the switcher and the
// screens agree on when the page is blocked.
import type { LinkState } from "@/link";
import type { GrantPhase } from "./useLazyGrant";

export type BlockingScreen = "requesting" | "pending" | "revoked" | "account_blocked" | "key_changed" | "desktop_refused" | "policy" | "grant_failed";

/** The slice of the link state the decision needs. */
export type LinkSlice = Pick<LinkState, "status" | "blocked">;

/**
 * `login_required`, `web_outdated` and `desktop_outdated` are handled by the `/d/:desktopId` layout and
 * never block here. While the grant request is in flight, a `revoked` block is not a verdict.
 */
export function blockingScreen(link: LinkSlice, phase: GrantPhase, reconnected: boolean): BlockingScreen | null {
  if (phase === "revoked") return "revoked";
  switch (link.blocked) {
    case "revoked": {
      if (phase === "failed") return "grant_failed";
      const settled = phase === "posted" ? reconnected : false;
      return settled ? "revoked" : "requesting";
    }
    case "account_blocked":
    case "key_changed":
    case "desktop_refused":
    case "policy":
      return link.blocked;
    default:
      break;
  }
  if (link.blocked === null && link.status === "pending") return "pending";
  if (link.blocked === null && phase === "requesting") return "requesting";
  return null;
}

/**
 * The switcher hides while one of these shows. Unlike `blockingScreen` it needs no grant phase: a
 * `revoked` block always hides it, which is the right answer in every phase.
 */
export function isLinkBlocking(link: LinkSlice): boolean {
  if (link.blocked === "login_required" || link.blocked === "web_outdated" || link.blocked === "desktop_outdated") return false;
  return link.blocked !== null || link.status === "pending";
}
