// The W-22 blocking screen (architecture.md 5.6, features.md N-23): shown instead of the outlet when the
// desktop and this web app speak different protocol versions.
import type { LinkBlockedReason } from "@/link/store";
import { Button } from "@/ui/button";
import { Card, CardDescription, CardTitle } from "@/ui/card";

export type VersionBlockedReason = Extract<LinkBlockedReason, "web_outdated" | "desktop_outdated">;

export function isVersionBlocked(reason: LinkBlockedReason | null): reason is VersionBlockedReason {
  return reason === "web_outdated" || reason === "desktop_outdated";
}

export function LinkBlockedScreen({ reason }: { reason: VersionBlockedReason }) {
  const newer = reason === "web_outdated";
  return (
    <div role="alert" data-testid="link-blocked-screen" data-reason={reason} className="mx-auto flex w-full max-w-[480px] flex-col gap-4 px-5 py-10">
      <Card className="flex flex-col gap-3 p-5">
        <CardTitle>{newer ? "Reload to update the web app" : "Update Monoptah on your desktop"}</CardTitle>
        <CardDescription>
          {newer
            ? "Your desktop runs a newer version of Monoptah than this web app. Reload to get the latest web app."
            : "Your desktop runs an older version of Monoptah than this web app supports. Update Monoptah on your desktop, then try again."}
        </CardDescription>
        {newer ? (
          // W-12's service worker update prompt replaces this reload later.
          <Button onClick={() => location.reload()}>Reload</Button>
        ) : null}
      </Card>
    </div>
  );
}
