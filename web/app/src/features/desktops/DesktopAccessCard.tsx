// The "Access" card of the desktop settings page: the devices that asked to reach this desktop with their
// permission ("lower now", "request raise"), revoke access, and the way to add another device.
// Permission lowering is applied by the desktop at once; raising needs a confirmation on the desktop (R-15).
// The server never writes `permission`, so the card only shows what the desktop reported and what was requested.
import { Link } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { QueryBoundary } from "@/features/settings/QueryBoundary";
import { useAction } from "@/features/settings/useAction";
import { formatRelative } from "@/features/settings/format";
import { cn } from "@/lib/utils";
import { displayFingerprint } from "@/link/fingerprint";
import { Badge } from "@/ui/badge";
import { Button, buttonVariants } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Notice } from "@/ui/Notice";
import { SETTINGS_ANCHOR, SETTINGS_DIVIDER_PLAIN, SettingsCard } from "@/ui/SettingsCard";
import { revokeGrant, setRequestedPermission, useDesktopGrants, type Grant } from "./api";
import { grantStateLabel, permissionLabel, safeName } from "./format";
import { desktopErrorMessage, Fingerprint } from "./parts";
import { useThisDevice } from "./thisDevice";

export function DesktopAccessCard({ desktopId, name }: { desktopId: string; name: string }) {
  const grants = useDesktopGrants(desktopId);
  const thisDevice = useThisDevice();
  return (
    <SettingsCard
      id="desktop-access"
      className={SETTINGS_ANCHOR}
      title="Access"
      footnote="Lowering to read only applies at once. Raising to control needs a confirmation on the desktop."
    >
      <QueryBoundary query={grants} label="devices">
        {(data) =>
          data.grants.length === 0 ? (
            <p className="px-1.5 py-3 text-small text-muted-text">No device has asked to reach this desktop yet.</p>
          ) : (
            <GrantList desktopId={desktopId} desktopName={name} grants={data.grants} thisDeviceId={thisDevice?.deviceId ?? null} />
          )
        }
      </QueryBoundary>
      <AddDeviceBlock />
    </SettingsCard>
  );
}

function GrantList({ desktopId, desktopName, grants, thisDeviceId }: { desktopId: string; desktopName: string; grants: Grant[]; thisDeviceId: string | null }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const revoke = useAction(desktopErrorMessage);
  const [busyDevice, setBusyDevice] = useState<string | null>(null);
  const [target, setTarget] = useState<Grant | null>(null);
  const ordered = [...grants].sort(
    (a, b) => Number(b.deviceId === thisDeviceId) - Number(a.deviceId === thisDeviceId) || Number(a.state === "revoked") - Number(b.state === "revoked") || b.updatedAt - a.updatedAt,
  );

  async function change(grant: Grant, permission: "read" | "control") {
    setBusyDevice(grant.deviceId);
    await action.run(() => setRequestedPermission(queryClient, desktopId, grant.deviceId, permission));
    setBusyDevice(null);
  }

  async function confirmRevoke() {
    if (!target) return;
    const ok = await revoke.run(() => revokeGrant(queryClient, desktopId, target.deviceId));
    if (ok) setTarget(null);
  }

  const targetName = target ? safeName(target.deviceName, "this device") : "";
  const targetFingerprint = target ? displayFingerprint(target.fingerprint) : "";

  return (
    <>
      <ul className="m-0 list-none p-0">
        {ordered.map((grant) => (
          <GrantRow
            key={grant.deviceId}
            grant={grant}
            isThis={grant.deviceId === thisDeviceId}
            busy={busyDevice === grant.deviceId}
            onChange={(permission) => void change(grant, permission)}
            onRevoke={() => {
              revoke.reset();
              setTarget(grant);
            }}
          />
        ))}
      </ul>
      {action.error ? <Notice variant="error" className="mx-1.5 mb-2">{action.error}</Notice> : null}
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(open) => {
          if (!open) setTarget(null);
        }}
        title={`Revoke access for ${targetName}?`}
        description={
          <>
            {targetName} will no longer be able to reach {desktopName}. It can ask for access again only after you forget the device.
            <span className="mt-2 block text-meta">Device fingerprint: {targetFingerprint}</span>
          </>
        }
        confirmLabel="Revoke access"
        destructive
        busy={revoke.busy}
        error={revoke.error}
        notice={revoke.notice}
        onConfirm={() => void confirmRevoke()}
      />
    </>
  );
}

function GrantRow({ grant, isThis, busy, onChange, onRevoke }: { grant: Grant; isThis: boolean; busy: boolean; onChange: (permission: "read" | "control") => void; onRevoke: () => void }) {
  const name = safeName(grant.deviceName, "Unnamed device");
  const revoked = grant.state === "revoked";
  const requested = grant.requestedPermission;
  const waiting = requested !== null && requested !== grant.permission;
  const canLower = grant.state === "confirmed" && grant.permission === "control" && !(waiting && requested === "read");
  const canRaise = grant.state === "confirmed" && grant.permission === "read" && !(waiting && requested === "control");

  return (
    <li className={cn("relative flex flex-wrap items-center gap-x-3 gap-y-2 rounded-row px-1.5 py-2", SETTINGS_DIVIDER_PLAIN)}>
      <div className="min-w-0 flex-[1_1_14rem] wrap-anywhere">
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-row text-content">
          <span className="min-w-0 wrap-anywhere">{name}</span>
          <span className="inline-flex flex-wrap items-center gap-1.5">
            {isThis ? <Badge variant="outline">This device</Badge> : null}
            <Badge variant={revoked ? "destructive" : grant.state === "pending" ? "outline" : "default"}>{grantStateLabel(grant.state)}</Badge>
          </span>
        </span>
        <Fingerprint hex={grant.fingerprint} className="block" />
        {revoked ? null : (
          <span className="block text-meta text-muted-text">
            {grant.state === "pending" ? "Waiting for confirmation on the desktop" : `Permission: ${permissionLabel(grant.permission)}`}
            {grant.confirmedAt !== null ? `, confirmed ${formatRelative(grant.confirmedAt)}` : ""}
          </span>
        )}
        {waiting && requested ? (
          <span className="block text-meta text-caution">
            {requested === "control" ? "Requested: Control, confirm on the desktop." : "Requested: Read only, being applied by the desktop."}
          </span>
        ) : null}
      </div>
      {revoked ? null : (
        <div className="flex flex-wrap gap-2">
          {canLower ? (
            <Button type="button" variant="outline" size="sm" disabled={busy} aria-label={`Lower ${name} to read only`} onClick={() => onChange("read")}>
              Lower to read only
            </Button>
          ) : null}
          {canRaise ? (
            <Button type="button" variant="outline" size="sm" disabled={busy} aria-label={`Request control for ${name}`} onClick={() => onChange("control")}>
              Request control
            </Button>
          ) : null}
          <Button type="button" variant="outline" size="sm" className={cn("text-deletions")} aria-label={`Revoke access for ${name}`} onClick={onRevoke}>
            Revoke access
          </Button>
        </div>
      )}
    </li>
  );
}

/** The target is the install page: it explains how to open the web app on another device. */
function AddDeviceBlock() {
  return (
    <div className={cn("relative flex flex-col gap-2 px-1.5 py-3", SETTINGS_DIVIDER_PLAIN)}>
      <span className="text-row text-content">Add another device</span>
      <p className="m-0 text-meta text-muted-text">
        Open this web app on the other device, sign in with your passkey and open this desktop. It then asks for confirmation on the desktop.
      </p>
      <Link to="/settings/install" className={cn(buttonVariants({ variant: "outline" }), "w-full sm:w-auto sm:self-start")}>
        Open the install guide
      </Link>
    </div>
  );
}
