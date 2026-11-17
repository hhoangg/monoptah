import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { desktopsQuery } from "@/features/accounts/queries";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Slot } from "@/ui/slots";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { DesktopLinkError, forgetDesktopLocally, useDesktopLink, type DesktopLink, type DesktopLinkStatus } from "./desktopLink";
import { workspaceQuery } from "./desktopQueries";
import { formatRelative } from "./format";
import { NotificationsSection } from "./NotificationsSection";
import { ProjectsPicker } from "./ProjectsPicker";
import { QueryBoundary } from "./QueryBoundary";
import { SettingsLayout } from "./SettingsLayout";
import { useAction } from "./useAction";

const LINK_LABELS: Record<DesktopLinkStatus, string> = {
  connecting: "Connecting...",
  connected: "Connected",
  reconnecting: "Reconnecting...",
  offline: "Desktop offline",
  failed: "Connection failed",
};

const NOT_CONNECTED = "Not connected";

const plural = (n: number, noun: string): string => `${n} ${noun}${n === 1 ? "" : "s"}`;

function ProjectsRow({ desktopId, link, onOpen }: { desktopId: string; link: DesktopLink; onOpen: () => void }) {
  const workspace = useQuery(workspaceQuery(desktopId, link));
  const counts = workspace.data ? `${plural(workspace.data.projects.length, "project")}, ${plural(workspace.data.monos.length, "Mono")}` : "Choose what this browser can see";
  return <SettingsRow icon="folder" title="Projects on this device" description={counts} onClick={onOpen} />;
}

/** Per-desktop settings (W-11, F-86): status, notifications, projects picker, Reconnect, remove. */
export function DesktopSettingsScreen({ desktopId }: { desktopId: string }) {
  const desktops = useQuery(desktopsQuery);
  return (
    <SettingsLayout>
      <QueryBoundary query={desktops} label="this desktop">
        {({ desktops: list }) => {
          const desktop = list.find((entry) => entry.id === desktopId);
          if (!desktop) {
            return (
              <>
                <PageHeader title="Desktop settings" />
                <Notice variant="error">This desktop is not on your account.</Notice>
                <SettingsCard>
                  <SettingsRow icon="computer" title="Back to your desktops" to="/desktops" />
                </SettingsCard>
              </>
            );
          }
          return <DesktopSettings desktopId={desktopId} name={desktop.name} accountOnline={desktop.online} lastSeenAt={desktop.lastSeenAt} />;
        }}
      </QueryBoundary>
    </SettingsLayout>
  );
}

function DesktopSettings({ desktopId, name, accountOnline, lastSeenAt }: { desktopId: string; name: string; accountOnline: boolean; lastSeenAt: number | null }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { link, status } = useDesktopLink(desktopId);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const reconnect = useAction();
  const remove = useAction();
  const [reconnected, setReconnected] = useState(false);

  const stateLabel = !accountOnline ? "Desktop offline" : status ? LINK_LABELS[status] : NOT_CONNECTED;
  const offline = !accountOnline || status === "offline";

  async function onReconnect(): Promise<void> {
    if (!link) return;
    setReconnected(false);
    const ok = await reconnect.run(async () => {
      await link.reconnect();
      await queryClient.invalidateQueries({ queryKey: ["settings", "desktop", desktopId] });
      await queryClient.fetchQuery(workspaceQuery(desktopId, link));
    });
    setReconnected(ok);
  }

  async function onRemove(): Promise<void> {
    const ok = await remove.run(async () => {
      if (link && !offline) {
        try {
          await link.call("device.leave", {});
        } catch (caught) {
          // An unreachable desktop must not trap the browser: forget locally and say so.
          if (!(caught instanceof DesktopLinkError) || caught.code !== "desktop_offline") throw caught;
        }
      }
      await forgetDesktopLocally(desktopId);
      queryClient.removeQueries({ queryKey: ["settings", "desktop", desktopId] });
    });
    if (ok) {
      setConfirmOpen(false);
      await navigate({ to: "/desktops" });
    }
  }

  return (
    <>
      <PageHeader title="Desktop settings" description={name} />
      <SettingsCard title="Status">
        <SettingsRow
          icon={offline ? "computerOff" : "computer"}
          title={name}
          description={!accountOnline && lastSeenAt ? `Last seen ${formatRelative(lastSeenAt)}` : undefined}
          trailing={stateLabel}
        />
      </SettingsCard>
      {!link ? (
        <Notice variant="info" title="Not connected">
          This browser has no connection to the desktop yet, so projects and Reconnect are unavailable. Open the desktop from Desktops to connect.
        </Notice>
      ) : null}
      <NotificationsSection desktopId={desktopId} />
      <SettingsCard title="Projects">
        {link ? (
          <ProjectsRow desktopId={desktopId} link={link} onOpen={() => setPickerOpen(true)} />
        ) : (
          <SettingsRow icon="folder" title="Projects on this device" description="Needs a connection to the desktop" />
        )}
      </SettingsCard>
      <Slot name="desktop-settings-sections" desktopId={desktopId} />
      <SettingsCard title="Connection">
        <SettingsRow
          icon="refresh"
          title="Reconnect"
          description={reconnect.busy ? "Reconnecting..." : "Run the handshake again and refresh the project list"}
          onClick={link && !reconnect.busy ? () => void onReconnect() : undefined}
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
      <SettingsCard title="Remove" footnote="The desktop stays on your account until you revoke it in Desktops.">
        <SettingsRow
          icon="trash"
          title={<span className="text-deletions">Remove this desktop from this device</span>}
          description="Forget this desktop in this browser"
          onClick={() => {
            remove.reset();
            setConfirmOpen(true);
          }}
        />
      </SettingsCard>
      {link ? <ProjectsPicker desktopId={desktopId} link={link} open={pickerOpen} onOpenChange={setPickerOpen} mode="settings" /> : null}
      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        title="Remove this desktop from this device?"
        description={
          offline || !link
            ? `${name} is offline or not connected, so it will only be forgotten in this browser. To revoke this device's access on your account, use Desktops.`
            : `${name} will forget this browser and this browser will forget ${name}. You can pair again later.`
        }
        confirmLabel="Remove"
        destructive
        busy={remove.busy}
        error={remove.error}
        onConfirm={() => void onRemove()}
      />
    </>
  );
}
