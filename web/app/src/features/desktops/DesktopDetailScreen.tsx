// `/desktops/:id`: manage one desktop (U-01, N-09, N-10, N-11): rename, color, the devices with their
// permission ("lower now", "request raise"), revoke device, "add another device" and revoke desktop.
// Owner: WS-M. Permission lowering is applied by the desktop at once; raising needs a confirmation
// on the desktop (R-15). The server never writes `permission`, so the screen only shows what the
// desktop reported and what was requested.
import { Link, useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent, type ReactNode } from "react";
import { isApiError } from "@/features/accounts/api";
import { QueryBoundary } from "@/features/settings/QueryBoundary";
import { SettingsLayout } from "@/features/settings/SettingsLayout";
import { useAction } from "@/features/settings/useAction";
import { formatRelative } from "@/features/settings/format";
import { cn } from "@/lib/utils";
import { displayFingerprint } from "@/link/fingerprint";
import { Badge } from "@/ui/badge";
import { Button, buttonVariants } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Field } from "@/ui/Field";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard } from "@/ui/SettingsCard";
import {
  renameDesktop,
  revokeDesktop,
  revokeGrant,
  setDesktopColor,
  setRequestedPermission,
  useDesktop,
  useDesktopGrants,
  type DesktopDirectory,
  type Grant,
} from "./api";
import { DESKTOP_COLORS, findDesktopColor } from "./colors";
import { grantStateLabel, lastSeenLabel, permissionLabel, safeName, versionLabel } from "./format";
import { useKeyStatus } from "./keyStatus";
import { ColorDot, desktopErrorMessage, Fingerprint, FingerprintLines, PresenceBadge } from "./parts";
import { useThisDevice } from "./thisDevice";

export function DesktopDetailScreen({ id }: { id: string }) {
  const desktop = useDesktop(id);
  if (desktop.data) return <DesktopDetail desktop={desktop.data} />;
  if (desktop.isError) {
    const missing = isApiError(desktop.error, "not_found") || isApiError(desktop.error, "forbidden");
    return (
      <SettingsLayout>
        <PageHeader title={missing ? "Desktop not found" : "Could not load this desktop"} description={missing ? "It may have been revoked, or it belongs to another account." : undefined} />
        <div className="flex flex-wrap gap-2">
          {missing ? null : (
            <Button type="button" variant="outline" onClick={() => void desktop.refetch()}>
              Try again
            </Button>
          )}
          <Link to="/desktops" className={buttonVariants({ variant: "outline" })}>
            Back to desktops
          </Link>
        </div>
      </SettingsLayout>
    );
  }
  return (
    <SettingsLayout>
      <div className="flex min-h-13 items-center justify-center py-8">
        <DelayedSpinner label="Loading desktop" />
      </div>
    </SettingsLayout>
  );
}

function DesktopDetail({ desktop }: { desktop: DesktopDirectory }) {
  const name = safeName(desktop.name, "Unnamed desktop");
  const version = versionLabel(desktop.os, desktop.appVersion);
  return (
    <SettingsLayout>
      <Link to="/desktops" className="-ml-2 inline-flex min-h-11 items-center gap-1 self-start rounded-row px-2 text-small text-brand-text hover:bg-selection pointer-fine:min-h-9">
        <Icon name="chevronRight" size={16} className="rotate-180" />
        All desktops
      </Link>
      <PageHeader
        title={name}
        description={
          <span className="flex flex-wrap items-center gap-2">
            {desktop.color === null ? null : <ColorDot color={desktop.color} />}
            <PresenceBadge online={desktop.online} />
            {desktop.online ? null : <span className="text-small">{lastSeenLabel(desktop.lastSeenAt, desktop.online)}</span>}
          </span>
        }
        actions={
          <Link to="/d/$desktopId" params={{ desktopId: desktop.id }} className={buttonVariants({ size: "sm" })}>
            Open desktop
          </Link>
        }
      />
      <SettingsCard title="About">
        <dl className="m-0 flex flex-col gap-2 px-1.5 py-2">
          {version ? <Detail label="System and version">{version}</Detail> : null}
          <KeyDetail desktop={desktop} />
        </dl>
      </SettingsCard>
      <RenameSection desktop={desktop} />
      <ColorSection desktop={desktop} />
      <DevicesSection desktop={desktop} name={name} />
      <AddDeviceSection />
      <DangerZone desktop={desktop} name={name} />
    </SettingsLayout>
  );
}

/** The key of this desktop as THIS device knows it: the pin, or the locally computed one; never the server's claim. */
function KeyDetail({ desktop }: { desktop: DesktopDirectory }) {
  const status = useKeyStatus(desktop.id, desktop.publicKey);
  switch (status.kind) {
    case "loading":
      return <Detail label="Fingerprint">Checking...</Detail>;
    case "invalid":
      return <Detail label="Fingerprint">The key of this desktop could not be read.</Detail>;
    case "pinned":
      return (
        <Detail label="Pinned on this device">
          <Fingerprint hex={status.pinned} />
        </Detail>
      );
    case "changed":
      return (
        <>
          <Detail label="Pinned on this device">
            <FingerprintLines hex={status.pinned} />
          </Detail>
          <Notice variant="error" title="Key changed since this device pinned it">
            The directory now serves a different key, with this fingerprint:
            <FingerprintLines hex={status.directory} className="mt-1" />
            Do not trust it yet. On your desktop, open Settings, then Web app, then "Verify on another device", and open that link on this device.
          </Notice>
        </>
      );
    case "unpinned":
      return (
        <Detail label="Reported by the server, not yet verified on this device">
          <Fingerprint hex={status.computed} />
        </Detail>
      );
  }
}

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-meta text-muted-text">{label}</dt>
      <dd className="m-0 min-w-0 text-body wrap-anywhere text-content">{children}</dd>
    </div>
  );
}

// --- Rename ---

function RenameSection({ desktop }: { desktop: DesktopDirectory }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const [draft, setDraft] = useState(desktop.name);
  // The name changed elsewhere (another tab or device, the 15 s refetch): follow it unless the field has edits.
  const [seenName, setSeenName] = useState(desktop.name);
  if (desktop.name !== seenName) {
    setSeenName(desktop.name);
    if (draft === seenName) setDraft(desktop.name);
  }
  const [saved, setSaved] = useState(false);
  const trimmed = draft.trim();
  const tooLong = [...trimmed].length > 64;
  const validation = trimmed === "" ? "Enter a name." : tooLong ? "Use 64 characters or fewer." : null;
  const unchanged = trimmed === desktop.name;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (validation || unchanged) return;
    setSaved(false);
    const ok = await action.run(() => renameDesktop(queryClient, desktop.id, trimmed));
    if (ok) {
      setDraft(trimmed);
      setSaved(true);
    }
  }

  return (
    <SettingsCard title="Name">
      <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-3 px-1.5 py-2" noValidate>
        <Field
          label="Desktop name"
          value={draft}
          maxLength={200}
          autoComplete="off"
          onChange={(event) => {
            setDraft(event.target.value);
            setSaved(false);
          }}
          error={draft !== desktop.name ? validation : null}
        />
        {action.error ? <Notice variant="error">{action.error}</Notice> : null}
        {saved ? <Notice variant="info">Name saved.</Notice> : null}
        <Button type="submit" variant="outline" className="w-full sm:w-auto sm:self-start" disabled={action.busy || validation !== null || unchanged} aria-busy={action.busy}>
          Save name
        </Button>
      </form>
    </SettingsCard>
  );
}

// --- Color ---

function ColorSection({ desktop }: { desktop: DesktopDirectory }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const current = findDesktopColor(desktop.color)?.value ?? null;

  return (
    <SettingsCard title="Color">
      <div className="flex flex-col gap-3 px-1.5 py-2">
        <div role="radiogroup" aria-label="Desktop color" className="flex flex-wrap gap-2">
          {DESKTOP_COLORS.map((color) => (
            <label key={color.value} className="relative inline-flex size-11 cursor-pointer items-center justify-center">
              <input
                type="radio"
                name={`color-${desktop.id}`}
                value={color.value}
                checked={current === color.value}
                disabled={action.busy}
                aria-label={color.name}
                className="peer sr-only"
                onChange={() => void action.run(() => setDesktopColor(queryClient, desktop.id, color.value))}
              />
              <span
                aria-hidden="true"
                className="size-8 rounded-pill border-2 border-transparent peer-checked:border-content peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-brand peer-disabled:opacity-40"
                style={{ backgroundColor: color.value }}
              />
            </label>
          ))}
        </div>
        {action.error ? <Notice variant="error">{action.error}</Notice> : null}
      </div>
    </SettingsCard>
  );
}

// --- Devices and permissions ---

function DevicesSection({ desktop, name }: { desktop: DesktopDirectory; name: string }) {
  const grants = useDesktopGrants(desktop.id);
  const thisDevice = useThisDevice();
  return (
    <SettingsCard
      title="Devices"
      footnote="Lowering to read only applies at once. Raising to control needs a confirmation on the desktop."
    >
      <QueryBoundary query={grants} label="devices">
        {(data) =>
          data.grants.length === 0 ? (
            <p className="px-1.5 py-3 text-small text-muted-text">No device has asked to reach this desktop yet.</p>
          ) : (
            <GrantList desktopId={desktop.id} desktopName={name} grants={data.grants} thisDeviceId={thisDevice?.deviceId ?? null} />
          )
        }
      </QueryBoundary>
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
    <li className="relative flex flex-wrap items-center gap-x-3 gap-y-2 rounded-row px-1.5 py-2 before:absolute before:top-0 before:right-0 before:left-4 before:h-px before:bg-stroke first:before:hidden">
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

// --- Add another device ---

/** The target is the install page: it explains how to open the web app on another device. */
function AddDeviceSection() {
  return (
    <SettingsCard
      title="Add another device"
      footnote="Open this web app on the other device, sign in with your passkey and open this desktop. It then asks for confirmation on the desktop."
    >
      <div className="px-1.5 py-2">
        <Link to="/install" className={cn(buttonVariants({ variant: "outline" }), "w-full sm:w-auto")}>
          Open the install guide
        </Link>
      </div>
    </SettingsCard>
  );
}

// --- Revoke desktop ---

function DangerZone({ desktop, name }: { desktop: DesktopDirectory; name: string }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const action = useAction(desktopErrorMessage);
  const [open, setOpen] = useState(false);

  async function confirm() {
    // Leave the page before its queries are removed, so the deleted desktop is never refetched ("not found" flash).
    const ok = await action.run(() => revokeDesktop(queryClient, desktop.id, { afterRevoke: () => navigate({ to: "/desktops" }) }));
    if (ok) setOpen(false);
  }

  return (
    <SettingsCard title="Danger zone" footnote="Revoking removes this desktop from your account. Pair it again to use it from the web.">
      <div className="px-1.5 py-2">
        <Button
          type="button"
          variant="destructive"
          className="w-full sm:w-auto"
          onClick={() => {
            action.reset();
            setOpen(true);
          }}
        >
          Revoke desktop
        </Button>
      </div>
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
    </SettingsCard>
  );
}
