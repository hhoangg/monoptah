import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useLink } from "@/link/store";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Notice } from "@/ui/Notice";
import { SETTINGS_ANCHOR, SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import type { DesktopLink, DesktopLinkStatus } from "./desktopLink";
import { updateStatusQuery } from "./desktopQueries";
import { useAction } from "./useAction";

type RunningSession = { id: string; title: string };

/** How long the "updating" notice waits for the link to drop and come back before it gives up. */
export const UPDATE_WAIT_MS = 5 * 60_000;

/** While the link stays up after `updating`, `desktop.updateStatus` is read this often to catch `lastFailure`. */
export const UPDATE_POLL_MS = 3000;

function SessionList({ sessions }: { sessions: RunningSession[] }) {
  return (
    <>
      <span className="block">Restarting interrupts these running sessions:</span>
      {sessions.map((session) => (
        <span key={session.id} className="mt-1 block">
          {session.title}
        </span>
      ))}
    </>
  );
}

/** "Desktop app update" card: current and latest version, and a Control-only "Update and restart". */
export function DesktopUpdateSection({
  id,
  desktopId,
  link,
  status,
  pending = false,
}: {
  id?: string;
  desktopId: string;
  link: DesktopLink | null;
  status: DesktopLinkStatus | null;
  /** True while it is not yet known whether this browser uses the desktop: shows a neutral "Checking..." state. */
  pending?: boolean;
}) {
  const queryClient = useQueryClient();
  const permission = useLink(desktopId, (state) => state.permission);
  const knownVersion = useLink(desktopId, (state) => state.desktopVersion);
  const query = useQuery({ ...updateStatusQuery(desktopId, link as DesktopLink), enabled: link !== null });
  const action = useAction();
  const [dialogOpen, setDialogOpen] = useState(false);
  // Sessions the desktop named in `confirmation_required`; null until it answered that.
  const [askedSessions, setAskedSessions] = useState<RunningSession[] | null>(null);
  const [updating, setUpdating] = useState(false);
  const [upToDate, setUpToDate] = useState(false);

  const canControl = permission === "control";
  const data = query.data;
  const sessions = askedSessions ?? data?.runningSessions ?? [];

  // After `updating`: wait for the link to drop, then for it to be back, then read the new version.
  useEffect(() => {
    if (!updating || !link) return;
    let dropped = false;
    const finish = () => {
      setUpdating(false);
      void queryClient.invalidateQueries({ queryKey: ["settings", "desktop", desktopId, "update"] });
    };
    const check = () => {
      if (link.status !== "connected") dropped = true;
      else if (dropped) finish();
    };
    check();
    return link.subscribe(check);
  }, [updating, link, queryClient, desktopId]);

  // The install can fail before the restart without the link ever dropping: ask the desktop for it.
  useEffect(() => {
    if (!updating || !link) return;
    let active = true;
    let reading = false;
    const timer = setInterval(() => {
      if (reading || link.status !== "connected") return;
      reading = true;
      link
        .call("desktop.updateStatus", {})
        .then((result) => {
          if (!active || !result.lastFailure) return;
          queryClient.setQueryData(updateStatusQuery(desktopId, link).queryKey, result);
          setUpdating(false);
        })
        .catch(() => undefined)
        .finally(() => {
          reading = false;
        });
    }, UPDATE_POLL_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [updating, link, queryClient, desktopId]);

  useEffect(() => {
    if (!updating) return;
    const timer = setTimeout(() => {
      setUpdating(false);
      void queryClient.invalidateQueries({ queryKey: ["settings", "desktop", desktopId, "update"] });
    }, UPDATE_WAIT_MS);
    return () => clearTimeout(timer);
  }, [updating, queryClient, desktopId]);

  async function onConfirm(): Promise<void> {
    if (!link) return;
    let finished = false;
    await action.run(async () => {
      const result = await link.call("desktop.update", sessions.length > 0 ? { confirm: true } : {});
      if (result.status === "confirmation_required") {
        setAskedSessions(result.runningSessions ?? []);
        return;
      }
      finished = true;
      if (result.status === "updating") {
        // The desktop forgets the old failure when a new install starts; do the same for the cached status.
        const key = updateStatusQuery(desktopId, link).queryKey;
        queryClient.setQueryData(key, (old: typeof data) => (old ? { ...old, lastFailure: undefined } : old));
        setUpdating(true);
      } else {
        setUpToDate(true);
        await queryClient.invalidateQueries({ queryKey: ["settings", "desktop", desktopId, "update"] });
      }
    });
    if (finished) {
      setDialogOpen(false);
      setAskedSessions(null);
    }
  }

  const current = data?.current ?? knownVersion;
  const connected = status === "connected";
  const available = data?.updateAvailable === true;

  return (
    <SettingsCard id={id} className={SETTINGS_ANCHOR} title="Desktop app update">
      <SettingsRow icon="computer" title="Current version" trailing={current ?? (link || pending ? "Checking..." : "Unknown")} />
      <SettingsRow icon="refresh" title="Latest version" trailing={data ? data.latest : link || pending ? "Checking..." : "Unknown"} />
      {!link ? (
        <SettingsRow icon="computer" title="Update and restart" description={pending ? "Checking..." : "Needs a connection to the desktop"} />
      ) : (
        <>
          {query.isError ? (
            <Notice variant="error" className="my-1.5">
              Could not read the update status from the desktop.
            </Notice>
          ) : null}
          {data?.error ? (
            <Notice variant="info" className="my-1.5">
              The desktop could not check for updates: {data.error}
            </Notice>
          ) : null}
          {data?.lastFailure && !updating ? (
            <Notice variant="error" className="my-1.5" title="The desktop update failed">
              {data.lastFailure}
            </Notice>
          ) : null}
          {upToDate && !available ? (
            <Notice variant="info" className="my-1.5">
              The desktop is up to date.
            </Notice>
          ) : null}
          {updating ? (
            <Notice variant="info" className="my-1.5" title="Desktop is updating and restarting">
              {status === "reconnecting" || status === "offline" ? "The connection is down while the desktop restarts. It will reconnect on its own." : "The desktop is downloading and installing the update."}
            </Notice>
          ) : null}
          {!canControl ? (
            <Notice variant="info" className="my-1.5">
              Control access is needed to update the desktop from here.
            </Notice>
          ) : available && connected && !updating ? (
            <div className="px-1.5 py-2">
              <Button
                type="button"
                onClick={() => {
                  action.reset();
                  setAskedSessions(null);
                  setUpToDate(false);
                  setDialogOpen(true);
                }}
              >
                Update and restart
              </Button>
            </div>
          ) : null}
          {action.error && !dialogOpen ? (
            <Notice variant="error" className="my-1.5">
              {action.error}
            </Notice>
          ) : null}
        </>
      )}
      <ConfirmDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        title="Update and restart the desktop?"
        description={sessions.length > 0 ? <SessionList sessions={sessions} /> : `${data?.latest ?? "The new version"} will be downloaded and installed, then the desktop restarts and reconnects.`}
        confirmLabel="Update and restart"
        busy={action.busy}
        error={action.error}
        onConfirm={() => void onConfirm()}
      />
    </SettingsCard>
  );
}
