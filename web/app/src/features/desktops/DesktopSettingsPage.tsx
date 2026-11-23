// `/settings/desktops/:id`: every setting of one desktop on one page (it merges the old /desktops/:id and
// /d/:id/settings): General (status, about, name, color, avatar), Models, Notifications, Projects visible in this
// browser, Desktop app update, Access, Troubleshoot and Remove. The anchors (`desktop-*`) are the ones of settingsIndex.ts.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState, type ReactNode } from "react";
import { isApiError } from "@/features/accounts/api";
import { DesktopUpdateSection } from "@/features/settings/DesktopUpdateSection";
import { useDesktopLink, type DesktopLink, type DesktopLinkStatus } from "@/features/settings/desktopLink";
import { workspaceQuery } from "@/features/settings/desktopQueries";
import { formatRelative } from "@/features/settings/format";
import { NotificationsSection } from "@/features/settings/NotificationsSection";
import { ProjectsPicker } from "@/features/settings/ProjectsPicker";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { useAction } from "@/features/settings/useAction";
import { Button, buttonVariants } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { SETTINGS_ANCHOR, SETTINGS_DIVIDER_PLAIN, SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Slot } from "@/ui/slots";
import { setDesktopColor, useDesktop, type DesktopDirectory } from "./api";
import { DESKTOP_COLORS, findDesktopColor } from "./colors";
import { DesktopAccessCard } from "./DesktopAccessCard";
import { DesktopAvatarSection } from "./DesktopAvatarSection";
import { DesktopRemoveCard } from "./DesktopRemoveCard";
import { wasGrantRequested } from "./storage";
import { useThisDeviceQuery } from "./thisDevice";
import { lastSeenLabel, safeName, versionLabel } from "./format";
import { useKeyStatus } from "./keyStatus";
import { ColorDot, desktopErrorMessage, Fingerprint, FingerprintLines, PresenceBadge } from "./parts";
import { cn } from "@/lib/utils";
import { RenameDesktopForm } from "./RenameDesktopForm";

const BACK = { to: "/settings/desktops", label: "All desktops" } as const;

const LINK_LABELS: Record<DesktopLinkStatus, string> = {
  connecting: "Connecting...",
  connected: "Connected",
  reconnecting: "Reconnecting...",
  offline: "Desktop offline",
  failed: "Connection failed",
};

const NOT_CONNECTED = "Not connected";
const PROJECTS_TITLE = "Projects visible in this browser";
const PROJECTS_CHOOSE = "Choose projects";


const plural = (n: number, noun: string): string => `${n} ${noun}${n === 1 ? "" : "s"}`;

export function DesktopSettingsPage({ id }: { id: string }) {
  const desktop = useDesktop(id);
  if (desktop.data) return <DesktopSettings desktop={desktop.data} />;
  if (desktop.isError) {
    const missing = isApiError(desktop.error, "not_found") || isApiError(desktop.error, "forbidden");
    return (
      <SettingsPage
        title={missing ? "Desktop not found" : "Could not load this desktop"}
        description={missing ? "It may have been revoked, or it belongs to another account." : undefined}
        back={BACK}
      >
        <div className="flex flex-wrap gap-2">
          {missing ? null : (
            <Button type="button" variant="outline" onClick={() => void desktop.refetch()}>
              Try again
            </Button>
          )}
          <Link to="/settings/desktops" className={buttonVariants({ variant: "outline" })}>
            Back to desktops
          </Link>
        </div>
      </SettingsPage>
    );
  }
  return (
    <SettingsPage title="Desktop" back={BACK}>
      <div className="flex min-h-13 items-center justify-center py-8">
        <DelayedSpinner label="Loading desktop" />
      </div>
    </SettingsPage>
  );
}

function DesktopSettings({ desktop }: { desktop: DesktopDirectory }) {
  const id = desktop.id;
  const name = safeName(desktop.name, "Unnamed desktop");
  const queryClient = useQueryClient();
  // Viewing settings must not connect a desktop this browser never used: connecting registers the device and posts a
  // grant request, which would show up as a pending access request on the desktop. So the page attaches to the link
  // (and mounts the Models slot, which opens its own link) only when this browser asked for access before. The device
  // loads asynchronously; until then the page shows neither the "not connected" notice nor the placeholders.
  const deviceQuery = useThisDeviceQuery();
  const thisDevice = deviceQuery.data ?? null;
  const deviceKnown = !deviceQuery.isPending;
  const used = thisDevice?.deviceId ? wasGrantRequested(id, thisDevice.deviceId) : false;
  const { link, status } = useDesktopLink(id, { enabled: used });
  const [pickerOpen, setPickerOpen] = useState(false);
  const reconnect = useAction();
  const [reconnected, setReconnected] = useState(false);

  const stateLabel = !desktop.online ? "Desktop offline" : status ? LINK_LABELS[status] : deviceKnown ? NOT_CONNECTED : "Checking...";
  const offline = !desktop.online || status === "offline" || status === "failed";

  async function onReconnect(): Promise<void> {
    if (!link) return;
    setReconnected(false);
    const ok = await reconnect.run(async () => {
      await link.reconnect();
      await queryClient.invalidateQueries({ queryKey: ["settings", "desktop", id] });
      await queryClient.fetchQuery(workspaceQuery(id, link));
    });
    setReconnected(ok);
  }

  return (
    <SettingsPage
      title={name}
      description={
        <span className="flex flex-wrap items-center gap-2">
          {desktop.color === null ? null : <ColorDot color={desktop.color} />}
          <PresenceBadge online={desktop.online} />
          {desktop.online ? null : <span className="text-small">{lastSeenLabel(desktop.lastSeenAt, desktop.online)}</span>}
        </span>
      }
      actions={
        <Link to="/d/$desktopId" params={{ desktopId: id }} className={buttonVariants({ size: "sm" })}>
          Open desktop
        </Link>
      }
      back={BACK}
    >
      <SettingsCard id="desktop-general" className={SETTINGS_ANCHOR} title="General">
        <GeneralBlock id="desktop-status" title="Status">
          <SettingsRow
            icon={offline ? "computerOff" : "computer"}
            title={name}
            description={!desktop.online && desktop.lastSeenAt ? `Last seen ${formatRelative(desktop.lastSeenAt)}` : undefined}
            trailing={stateLabel}
            className="before:hidden"
          />
        </GeneralBlock>
        <AboutBlock desktop={desktop} />
        <GeneralBlock id="desktop-name" title="Name">
          <RenameDesktopForm desktop={desktop} />
        </GeneralBlock>
        <ColorBlock desktop={desktop} />
        <GeneralBlock id="desktop-avatar" title="Avatar">
          <DesktopAvatarSection desktop={desktop} embedded />
        </GeneralBlock>
      </SettingsCard>
      {deviceKnown && !link ? (
        <Notice variant="info" title="Not connected">
          This browser has no connection to the desktop yet, so projects and Reconnect are unavailable. Open the desktop to connect.
        </Notice>
      ) : null}
      {!deviceKnown ? (
        <div id="desktop-models" className={SETTINGS_ANCHOR}>
          <DelayedSpinner label="Loading connection" className="px-1.5 py-2" />
        </div>
      ) : used ? (
        <div id="desktop-models" className={cn(SETTINGS_ANCHOR, "empty:hidden")}>
          <Slot name="desktop-settings-sections" desktopId={id} />
        </div>
      ) : (
        <div id="desktop-models" className={SETTINGS_ANCHOR}>
          <SettingsCard title="Models">
            <SettingsRow icon="sparkle" title="Default model" description="Needs a connection to the desktop" />
          </SettingsCard>
        </div>
      )}
      <div id="desktop-notifications" className={SETTINGS_ANCHOR}>
        <NotificationsSection desktopId={id} />
      </div>
      <SettingsCard id="desktop-projects" className={SETTINGS_ANCHOR} title={PROJECTS_TITLE}>
        {link ? (
          <ProjectsRow desktopId={id} link={link} onOpen={() => setPickerOpen(true)} />
        ) : deviceKnown ? (
          <SettingsRow icon="folder" title={PROJECTS_CHOOSE} description="Needs a connection to the desktop" />
        ) : (
          <DelayedSpinner label="Loading connection" className="px-1.5 py-2" />
        )}
      </SettingsCard>
      <DesktopUpdateSection id="desktop-update" desktopId={id} link={link} status={status} pending={!deviceKnown} />
      <DesktopAccessCard desktopId={id} name={name} />
      <SettingsCard id="desktop-troubleshoot" className={SETTINGS_ANCHOR} title="Troubleshoot">
        <SettingsRow
          icon="refresh"
          title="Reconnect"
          description={reconnect.busy ? "Reconnecting..." : "Run the handshake again and refresh the project list"}
          onClick={link ? () => void onReconnect() : undefined}
          disabled={reconnect.busy}
        />
        {reconnect.error ? (
          <Notice variant="error" className="my-1.5">
            {reconnect.error}
          </Notice>
        ) : null}
        {reconnected ? (
          <Notice variant="info" className="my-1.5">
            Reconnected and refreshed from the desktop.
          </Notice>
        ) : null}
      </SettingsCard>
      <DesktopRemoveCard desktopId={id} name={name} link={link} offline={offline} />
      {link ? <ProjectsPicker desktopId={id} link={link} open={pickerOpen} onOpenChange={setPickerOpen} mode="settings" /> : null}
    </SettingsPage>
  );
}

/** One block of the General card: a small label, then its content, with an inset divider above all but the first. */
function GeneralBlock({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <div id={id} className={`${SETTINGS_ANCHOR} relative flex flex-col pt-3 pb-1 first:pt-1 ${SETTINGS_DIVIDER_PLAIN}`}>
      <h3 className="px-1.5 text-label font-medium text-muted-text">{title}</h3>
      {children}
    </div>
  );
}

function ProjectsRow({ desktopId, link, onOpen }: { desktopId: string; link: DesktopLink; onOpen: () => void }) {
  const workspace = useQuery(workspaceQuery(desktopId, link));
  const counts = workspace.data ? `${plural(workspace.data.projects.length, "project")}, ${plural(workspace.data.monos.length, "Mono")}` : "Choose what this browser can see";
  return <SettingsRow icon="folder" title={PROJECTS_CHOOSE} description={counts} onClick={onOpen} />;
}

// --- About ---

function AboutBlock({ desktop }: { desktop: DesktopDirectory }) {
  const version = versionLabel(desktop.os, desktop.appVersion);
  return (
    <GeneralBlock id="desktop-about" title="About">
      <dl className="m-0 flex flex-col gap-2 px-1.5 py-2">
        {version ? <Detail label="System and version">{version}</Detail> : null}
        <KeyDetail desktop={desktop} />
      </dl>
    </GeneralBlock>
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

// --- Color ---

function ColorBlock({ desktop }: { desktop: DesktopDirectory }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const current = findDesktopColor(desktop.color)?.value ?? null;

  return (
    <GeneralBlock id="desktop-color" title="Color">
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
    </GeneralBlock>
  );
}
