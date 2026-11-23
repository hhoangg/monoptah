// The "Remove" card of the desktop settings page: two clearly scoped actions.
// "Forget in this browser" only affects this browser; "Revoke from account" removes the desktop from the account.
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { DesktopLinkError, forgetDesktopLocally, type DesktopLink } from "@/features/settings/desktopLink";
import { useAction } from "@/features/settings/useAction";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { SETTINGS_ANCHOR, SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { revokeDesktop } from "./api";
import { desktopErrorMessage } from "./parts";

export function DesktopRemoveCard({ desktopId, name, link, offline }: { desktopId: string; name: string; link: DesktopLink | null; offline: boolean }) {
  return (
    <SettingsCard
      id="desktop-remove"
      className={SETTINGS_ANCHOR}
      title="Remove"
      footnote="Forget in this browser only affects this browser: the desktop stays on your account and other browsers keep their access. Revoke from account removes the desktop and every device's access for good; pair it again to use it from the web."
    >
      <ForgetInBrowser desktopId={desktopId} name={name} link={link} offline={offline} />
      <RevokeFromAccount desktopId={desktopId} name={name} />
    </SettingsCard>
  );
}

function ForgetInBrowser({ desktopId, name, link, offline }: { desktopId: string; name: string; link: DesktopLink | null; offline: boolean }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const remove = useAction();
  const [confirmOpen, setConfirmOpen] = useState(false);
  // True after `device.leave` failed for a reason other than "offline": the user may forget locally.
  const [leaveFailed, setLeaveFailed] = useState(false);

  /** `forceLocal` skips the desktop call: the user chose to forget this desktop on this device only. */
  async function onRemove(forceLocal = false): Promise<void> {
    const ok = await remove.run(async () => {
      if (link && !offline && !forceLocal) {
        try {
          await link.call("device.leave", {});
        } catch (caught) {
          // An unreachable desktop must not trap the browser: forget locally and say so. Any other
          // failure stops here and offers "Remove from this device anyway" instead.
          if (!(caught instanceof DesktopLinkError) || caught.code !== "desktop_offline") {
            setLeaveFailed(true);
            throw caught;
          }
        }
      }
      await forgetDesktopLocally(desktopId);
      queryClient.removeQueries({ queryKey: ["settings", "desktop", desktopId] });
    });
    if (ok) {
      setConfirmOpen(false);
      await navigate({ to: "/settings/desktops" });
    }
  }

  return (
    <>
      <SettingsRow
        icon="computerOff"
        title={<span className="text-deletions">Forget in this browser</span>}
        description="This browser stops using this desktop. It stays on your account."
        onClick={() => {
          remove.reset();
          setLeaveFailed(false);
          setConfirmOpen(true);
        }}
      />
      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        title="Remove this desktop from this device?"
        description={
          leaveFailed
            ? `${name} could not be told to forget this browser. You can still remove it from this device only; it may keep listing this browser until you revoke access in Desktops.`
            : offline || !link
            ? `${name} is offline or not connected, so it will only be forgotten in this browser. To revoke this device's access on your account, use Desktops.`
            : `${name} will forget this browser and this browser will forget ${name}. You can pair again later.`
        }
        confirmLabel="Remove"
        destructive
        busy={remove.busy}
        error={remove.error}
        onConfirm={() => void onRemove()}
        secondary={leaveFailed ? { label: "Remove from this device anyway", onClick: () => void onRemove(true) } : undefined}
      />
    </>
  );
}

function RevokeFromAccount({ desktopId, name }: { desktopId: string; name: string }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const action = useAction(desktopErrorMessage);
  const [open, setOpen] = useState(false);

  async function confirm() {
    // Leave the page before its queries are removed, so the deleted desktop is never refetched ("not found" flash).
    const ok = await action.run(() => revokeDesktop(queryClient, desktopId, { afterRevoke: () => navigate({ to: "/settings/desktops" }) }));
    if (ok) setOpen(false);
  }

  return (
    <>
      <SettingsRow
        icon="trash"
        title={<span className="text-deletions">Revoke from account</span>}
        description="Removes this desktop and every device's access from your account, in every browser. Needs your passkey."
        onClick={() => {
          action.reset();
          setOpen(true);
        }}
      />
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`Revoke ${name}?`}
        description={`${name} and every device's access to it are removed from your account. You will be asked to confirm with your passkey.`}
        confirmLabel="Revoke desktop"
        destructive
        busy={action.busy}
        error={action.error}
        notice={action.notice}
        onConfirm={() => void confirm()}
      />
    </>
  );
}
