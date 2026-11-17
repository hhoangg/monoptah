// Route of /settings. Owner: WS-S (workstreams.md 3.1).
import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { ForcedRegenerate } from "@/features/accounts/ForcedRegenerate";
import { meQuery } from "@/features/accounts/queries";
import { SettingsScreen } from "@/features/settings/SettingsScreen";
import { DelayedSpinner } from "@/ui/DelayedSpinner";

export const Route = createFileRoute("/settings/")({
  component: Page,
});

function Page() {
  const me = useQuery(meQuery);
  if (!me.data) return <DelayedSpinner label="Loading" className="m-5" />;
  // The auth guard sends every route here while the recovery codes are unconfirmed (Task 3 gate).
  if (me.data.recoveryAckAt === null) return <ForcedRegenerate handle={me.data.user.handle} />;
  return <SettingsScreen handle={me.data.user.handle} displayName={me.data.user.displayName} />;
}
