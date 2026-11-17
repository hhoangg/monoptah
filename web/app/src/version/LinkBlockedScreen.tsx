// Placeholder of the W-22 blocking screen (architecture.md 5.6); Task 6 of WS-L completes it.
// Shown instead of the outlet when the desktop and this web app speak different protocol versions.
import type { LinkBlockedReason } from "@/link/store";

export type VersionBlockedReason = Extract<LinkBlockedReason, "proto_desktop_newer" | "proto_desktop_older">;

export function isVersionBlocked(reason: LinkBlockedReason | null): reason is VersionBlockedReason {
  return reason === "proto_desktop_newer" || reason === "proto_desktop_older";
}

export function LinkBlockedScreen({ reason }: { reason: VersionBlockedReason }) {
  const text = reason === "proto_desktop_newer" ? "Reload to update the web app" : "Update Monoptah on your desktop";
  return (
    <div role="alert" data-testid="link-blocked-screen" data-reason={reason}>
      <p>{text}</p>
    </div>
  );
}
