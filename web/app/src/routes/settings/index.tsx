// Route of /settings. Owner: WS-S (workstreams.md 3.1).
import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { ForcedRegenerate } from "@/features/accounts/ForcedRegenerate";
import { hasLowCodes } from "@/features/accounts/lowCodes";
import { useRecoveryCodesShown } from "@/features/accounts/pendingRecoveryCodes";
import { meQuery } from "@/features/accounts/queries";
import { SettingsScreen } from "@/features/settings/SettingsScreen";
import { DelayedSpinner } from "@/ui/DelayedSpinner";

export const Route = createFileRoute("/settings/")({
  component: Page,
});

function Page() {
  const me = useQuery(meQuery);
  const codesShown = useRecoveryCodesShown();
  if (!me.data) return <DelayedSpinner label="Loading" className="m-5" />;
  // The auth guard sends every route here while the recovery codes are unconfirmed (Task 3 gate).
  if (!codesShown && (me.data.recoveryAckAt === null || hasLowCodes(me.data.user.handle))) return <ForcedRegenerate handle={me.data.user.handle} />;
  return <SettingsScreen handle={me.data.user.handle} displayName={me.data.user.displayName} role={me.data.user.role} />;
}
