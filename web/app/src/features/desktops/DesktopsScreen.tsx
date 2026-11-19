// `/desktops`: the desktop list, the "Pair a desktop" help with the optional paste field (F-02) and
// this account's devices (U-01, N-09, N-12). Owner: WS-M. Names and OS strings come from other
// machines: they are rendered as text only.
import { Link, useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { safeName, lastSeenLabel, versionLabel } from "./format";
import { Icon } from "@/ui/Icon";
import { forgetDevice, useDesktops, useDevices, type DesktopSummary, type DeviceSummary } from "./api";
import { desktopErrorMessage, ColorDot, Fingerprint, PresenceBadge } from "./parts";
import { parsePairingInput } from "./pairingLink";
import { RenameDesktopForm } from "./RenameDesktopForm";
import { useThisDevice } from "./thisDevice";
import { SettingsLayout } from "@/features/settings/SettingsLayout";
import { QueryBoundary } from "@/features/settings/QueryBoundary";
import { formatRelative } from "@/features/settings/format";
import { useAction } from "@/features/settings/useAction";
import { Badge } from "@/ui/badge";
import { Button, buttonVariants } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Field } from "@/ui/Field";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard } from "@/ui/SettingsCard";
import { cn } from "@/lib/utils";
import { displayFingerprint } from "@/link/fingerprint";

export function DesktopsScreen() {
  const desktops = useDesktops();
  return (
    <SettingsLayout>
      <PageHeader title="Desktops" description="Open a desktop to work with its sessions, or manage who can reach it." />
      <SettingsCard title="Your desktops">
        <QueryBoundary query={desktops} label="desktops">
          {(data) => (data.desktops.length === 0 ? <EmptyDesktops /> : <DesktopList desktops={data.desktops} />)}
        </QueryBoundary>
      </SettingsCard>
      <PairSection empty={desktops.data?.desktops.length === 0} />
      <DevicesSection />
    </SettingsLayout>
  );
}

const PAIR_STEPS = [
  "On the desktop, open Settings > Web app > Pair. It shows a link and a QR code.",
  "Scan the QR code with this device, or paste the link or its 8 character code below.",
  "Confirm the match on the desktop. It then shows up here.",
] as const;

function EmptyDesktops() {
  return (
    <div className="flex flex-col items-center gap-3 px-3 py-6 text-center">
      <Icon name="computer" size={40} className="text-faint" />
      <h2 className="text-headline font-semibold text-content">Pair your first desktop</h2>
      <p className="max-w-prose text-small text-muted-text">A desktop must be paired before you can reach its sessions from this device.</p>
      <ol className="m-0 flex max-w-prose list-decimal flex-col gap-1.5 pl-5 text-left text-small text-content">
        {PAIR_STEPS.map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ol>
    </div>
  );
}

function DesktopList({ desktops }: { desktops: DesktopSummary[] }) {
  return (
    <ul className="m-0 list-none p-0">
      {desktops.map((desktop) => (
        <DesktopRow key={desktop.id} desktop={desktop} />
      ))}
    </ul>
  );
}

function DesktopRow({ desktop }: { desktop: DesktopSummary }) {
  const [renaming, setRenaming] = useState(false);
  const name = safeName(desktop.name, "Unnamed desktop");
  const version = versionLabel(desktop.os, desktop.appVersion);
  return (
    <li className="relative flex flex-wrap items-center gap-x-2 gap-y-1 rounded-row px-1.5 py-1 before:absolute before:top-0 before:right-0 before:left-4 before:h-px before:bg-stroke first:before:hidden">
      <Link
        to="/d/$desktopId"
        params={{ desktopId: desktop.id }}
        className="flex min-h-13 min-w-0 flex-[1_1_12rem] items-center gap-3 rounded-row py-2 pl-1.5 text-left hover:bg-selection active:opacity-60"
      >
        {desktop.color === null ? null : <ColorDot color={desktop.color} />}
        <span className="min-w-0 flex-1 wrap-anywhere">
          <span className="flex flex-wrap items-center gap-2 text-row text-content">
            <span className="sr-only">Open</span> {name}
            <PresenceBadge online={desktop.online} />
            {desktop.pendingCount > 0 ? <Badge variant="brand">{`${desktop.pendingCount} to confirm`}</Badge> : null}
          </span>
          <span className="block text-meta text-muted-text">{[lastSeenLabel(desktop.lastSeenAt, desktop.online), version].filter(Boolean).join(" - ")}</span>
        </span>
        <Icon name="chevronRight" size={18} className="mr-1 shrink-0 text-faint" />
      </Link>
      <Link
        to="/desktops/$id"
        params={{ id: desktop.id }}
        aria-label={`Manage ${name}`}
        className={cn(buttonVariants({ variant: "ghost", size: "sm" }), "shrink-0 text-muted-text")}
      >
        Manage
      </Link>
      <Button type="button" variant="ghost" size="sm" aria-label={`Rename ${name}`} className="shrink-0 text-muted-text" onClick={() => setRenaming(true)}>
        Rename
      </Button>
      <Dialog open={renaming} onOpenChange={setRenaming}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Rename desktop</DialogTitle>
            <DialogDescription>Choose the name this account shows for {name}.</DialogDescription>
          </DialogHeader>
          <RenameDesktopForm desktop={desktop} onSaved={() => setRenaming(false)} />
        </DialogContent>
      </Dialog>
    </li>
  );
}

// --- Pair a desktop (F-02) ---

function PairSection({ empty }: { empty: boolean }) {
  const navigate = useNavigate();
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const target = parsePairingInput(value);
    if (!target) {
      setError("That is not a pairing link or code of this app. Paste the link shown on the desktop, or its 8 character code.");
      return;
    }
    setError(null);
    await navigate({ to: "/pair/$code", params: { code: target.code }, hash: target.fp ? `fp=${encodeURIComponent(target.fp)}` : undefined });
  }

  return (
    <SettingsCard
      title="Pair a desktop"
      footnote={empty ? undefined : "On the desktop, open Settings > Web app > Pair. It shows a link and a QR code. Scan the code with this device, or paste the link here."}
    >
      <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-3 px-1.5 py-2" noValidate>
        <Field
          label="Paste a pairing link"
          placeholder="Pairing link or code"
          value={value}
          onChange={(event) => {
            setValue(event.target.value);
            setError(null);
          }}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          error={error}
          hint="Paste the link or the 8 character code shown on the desktop."
        />
        <Button type="submit" className="w-full sm:w-auto sm:self-start" disabled={value.trim() === ""}>
          Continue
        </Button>
      </form>
    </SettingsCard>
  );
}

// --- This account's devices ---

function DevicesSection() {
  const devices = useDevices();
  const thisDevice = useThisDevice();
  return (
    <SettingsCard
      title="This account's devices"
      footnote="Browsers that have asked to reach a desktop. Forgetting a device removes its access on every desktop and deletes its notifications."
    >
      <QueryBoundary query={devices} label="devices">
        {(data) =>
          data.devices.length === 0 ? (
            <p className="px-1.5 py-3 text-small text-muted-text">No device has asked for access yet.</p>
          ) : (
            <DeviceList devices={data.devices} thisDeviceId={thisDevice?.deviceId ?? null} />
          )
        }
      </QueryBoundary>
    </SettingsCard>
  );
}

function DeviceList({ devices, thisDeviceId }: { devices: DeviceSummary[]; thisDeviceId: string | null }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const [target, setTarget] = useState<DeviceSummary | null>(null);
  const ordered = [...devices].sort((a, b) => Number(b.id === thisDeviceId) - Number(a.id === thisDeviceId) || (b.lastSeenAt ?? 0) - (a.lastSeenAt ?? 0));

  async function confirm() {
    if (!target) return;
    const ok = await action.run(() => forgetDevice(queryClient, target.id));
    if (ok) setTarget(null);
  }

  return (
    <>
      <ul className="m-0 list-none p-0">
        {ordered.map((device) => {
          const name = safeName(device.name, "Unnamed device");
          const isThis = device.id === thisDeviceId;
          return (
            <li
              key={device.id}
              className="relative flex flex-wrap items-center gap-x-3 gap-y-1 rounded-row px-1.5 py-2 before:absolute before:top-0 before:right-0 before:left-4 before:h-px before:bg-stroke first:before:hidden"
            >
              <span className="min-w-0 flex-[1_1_12rem] wrap-anywhere">
                <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-row text-content">
                  <span className="min-w-0 wrap-anywhere">{name}</span>
                  {isThis ? <Badge variant="outline">This device</Badge> : null}
                </span>
                <Fingerprint hex={device.fingerprint} className="block" />
                <span className="block text-meta text-muted-text">{device.lastSeenAt === null ? "Never used" : `Last seen ${formatRelative(device.lastSeenAt)}`}</span>
              </span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                aria-label={`Forget device ${name}`}
                onClick={() => {
                  action.reset();
                  setTarget(device);
                }}
              >
                Forget device
              </Button>
            </li>
          );
        })}
      </ul>
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(open) => {
          if (!open) setTarget(null);
        }}
        title={target ? `Forget ${safeName(target.name, "this device")}?` : "Forget device?"}
        description={
          <>
            {target?.id === thisDeviceId
              ? "This is the browser you are using. It loses access on every desktop, its notifications are removed, and its key, its pinned desktop keys and its saved desktop data are deleted here. It has to ask for access again."
              : "It loses access on every desktop and its notifications are removed. It has to ask for access again."}
            {target ? <span className="mt-2 block text-meta">Device fingerprint: {displayFingerprint(target.fingerprint)}</span> : null}
          </>
        }
        confirmLabel="Forget device"
        destructive
        busy={action.busy}
        error={action.error}
        notice={action.notice}
        onConfirm={() => void confirm()}
      />
    </>
  );
}
