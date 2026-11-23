// Layout route of /settings/*: the one settings shell (nav beside the page from 1024px, a list that pushes into a sub
// page below that). It also holds the forced recovery-codes gate: while the codes are unconfirmed the auth guard sends
// every route to /settings/profile and this layout shows the regenerate screen there instead of the shell.
import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Outlet } from "@tanstack/react-router";
import { ForcedRegenerate } from "@/features/accounts/ForcedRegenerate";
import { hasLowCodes } from "@/features/accounts/lowCodes";
import { useRecoveryCodesShown } from "@/features/accounts/pendingRecoveryCodes";
import { meQuery } from "@/features/accounts/queries";
import { SettingsShell } from "@/features/settings/SettingsShell";
import { DelayedSpinner } from "@/ui/DelayedSpinner";

export const Route = createFileRoute("/settings")({
  component: Layout,
});

function Layout() {
  const me = useQuery(meQuery);
  const codesShown = useRecoveryCodesShown();
  if (!me.data) return <DelayedSpinner label="Loading" className="m-5" />;
  if (!codesShown && (me.data.recoveryAckAt === null || hasLowCodes(me.data.user.handle))) return <ForcedRegenerate handle={me.data.user.handle} />;
  return (
    <SettingsShell>
      <Outlet />
    </SettingsShell>
  );
}
