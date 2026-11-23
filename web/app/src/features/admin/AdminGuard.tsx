import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { meQuery } from "@/features/accounts/queries";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { NOT_AVAILABLE } from "./copy";

/** A plain screen with no hint that an admin area exists. */
export function NotAvailable() {
  return (
    <SettingsPage title="Not available" description={NOT_AVAILABLE}>
      <Button asChild variant="outline" className="self-start">
        <Link to="/">Go to Monoptah</Link>
      </Button>
    </SettingsPage>
  );
}

/**
 * Admin routes need `role = "admin"` (security.md 4). A non-admin sees "Not available" and no admin
 * request is made: the children, which own every admin query, never mount. The server's 403 stays the
 * real gate and is handled in `AdminBoundary`.
 */
export function AdminGuard({ children }: { children: (user: { id: string; handle: string }) => ReactNode }) {
  const me = useQuery(meQuery);
  if (!me.data) return <DelayedSpinner label="Loading" className="m-5" />;
  if (me.data.user.role !== "admin") return <NotAvailable />;
  return <>{children(me.data.user)}</>;
}
