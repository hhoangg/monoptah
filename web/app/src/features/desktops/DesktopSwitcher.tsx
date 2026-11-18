// The `desktop-switcher` slot of `/d/:desktopId` (W-04). Owner: WS-M.
//
// A slim sticky bar (current desktop color dot, name, online dot) that opens the list of the
// account's desktops: a bottom sheet under 640 px, a dialog from 640 px (`ResponsiveOverlay`,
// design-system.md 11). Desktops are sorted by last use; choosing one opens `/d/:id`. "Request
// access" posts the grant of that one desktop on tap (never for all). The bar is hidden while a
// blocking link-state layer shows (any of them, including "Requesting access..."), and the last-used order is updated when the page mounts.
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { ensureDevice, useLinkDesktopId, useLinkState } from "@/link";
import { cn } from "@/lib/utils";
import { Button, buttonVariants } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { useAction } from "@/features/settings/useAction";
import { requestGrantAndRefresh, useDesktopGrants, useDesktops, type DesktopSummary } from "./api";
import { safeName } from "./format";
import { useBlockingLayerShown } from "./blockingLayer";
import { isLinkBlocking } from "./linkStateModel";
import { ColorDot, desktopErrorMessage } from "./parts";
import { markGrantRequested, readLastUsed, sortByLastUsed, touchLastUsed } from "./storage";
import { useThisDevice } from "./thisDevice";

function OnlineDot({ online }: { online: boolean }) {
  return (
    <span className="inline-flex items-center gap-1 text-caption text-muted-text">
      <span aria-hidden="true" className={cn("size-1.5 rounded-pill", online ? "bg-additions" : "bg-faint")} />
      {online ? "Online" : "Offline"}
    </span>
  );
}

export function DesktopSwitcher() {
  const desktopId = useLinkDesktopId();
  const status = useLinkState((state) => state.status);
  const blocked = useLinkState((state) => state.blocked);
  const linkName = useLinkState((state) => state.desktopName);
  const presence = useLinkState((state) => state.presence);
  const desktops = useDesktops();
  const layerShown = useBlockingLayerShown(desktopId);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    touchLastUsed(desktopId);
  }, [desktopId]);

  // Hidden while any blocking layer shows, including "Requesting access...".
  if (layerShown || isLinkBlocking({ status, blocked })) return null;

  const current = desktops.data?.desktops.find((item) => item.id === desktopId);
  const name = safeName(current?.name ?? linkName ?? "", "Desktop");
  // The live link knows better than the 15 s list: use its presence once it has one.
  const online = presence === null ? (current?.online ?? false) : presence === "online";

  return (
    <div className="sticky top-0 z-30 border-b border-stroke bg-background pt-[env(safe-area-inset-top)]">
      <div className="mx-auto flex min-h-11 w-full max-w-[1280px] items-center px-[max(0.75rem,env(safe-area-inset-left))] pr-[max(0.75rem,env(safe-area-inset-right))]">
        <button
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-label={`Switch desktop, current: ${name}`}
          onClick={() => setOpen(true)}
          className="flex min-h-11 min-w-0 max-w-full items-center gap-2 rounded-row px-2 text-label font-semibold text-content hover:bg-selection active:opacity-60 pointer-fine:min-h-9"
        >
          <ColorDot color={current?.color ?? null} />
          <span className="min-w-0 truncate">{name}</span>
          <OnlineDot online={online} />
          <Icon name="chevronDown" size={16} className="shrink-0 text-faint" />
        </button>
      </div>
      {open ? <SwitcherSheet currentId={desktopId} onClose={() => setOpen(false)} /> : null}
    </div>
  );
}

function SwitcherSheet({ currentId, onClose }: { currentId: string; onClose: () => void }) {
  const desktops = useDesktops();
  const thisDevice = useThisDevice();
  const order = useMemo(() => readLastUsed(), []);
  const sorted = useMemo(() => sortByLastUsed(desktops.data?.desktops ?? [], order), [desktops.data, order]);

  return (
    <ResponsiveOverlay
      open
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      title="Switch desktop"
      description="Your most recently used desktops come first."
      footer={
        <Link to="/desktops" onClick={onClose} className={buttonVariants({ variant: "outline" })}>
          Manage desktops
        </Link>
      }
    >
      {desktops.isError ? <Notice variant="error">Could not load your desktops.</Notice> : null}
      {desktops.isPending ? <p className="text-small text-muted-text">Loading desktops...</p> : null}
      <ul className="m-0 flex list-none flex-col p-0">
        {sorted.map((desktop) => (
          <SwitcherRow key={desktop.id} desktop={desktop} current={desktop.id === currentId} deviceId={thisDevice?.deviceId ?? null} onClose={onClose} />
        ))}
      </ul>
    </ResponsiveOverlay>
  );
}

function SwitcherRow({ desktop, current, deviceId, onClose }: { desktop: DesktopSummary; current: boolean; deviceId: string | null; onClose: () => void }) {
  const name = safeName(desktop.name, "Unnamed desktop");
  return (
    <li className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-stroke py-1 last:border-b-0">
      <Link
        to="/d/$desktopId"
        params={{ desktopId: desktop.id }}
        onClick={onClose}
        aria-current={current ? "true" : undefined}
        aria-label={current ? `${name}, current desktop` : `Open ${name}`}
        className={cn("flex min-h-11 min-w-0 flex-[1_1_12rem] items-center gap-3 rounded-row px-2 py-1.5 hover:bg-selection active:opacity-60", current && "bg-selection")}
      >
        <ColorDot color={desktop.color} />
        <span className="min-w-0 flex-1 text-row wrap-anywhere text-content">{name}</span>
        <OnlineDot online={desktop.online} />
        {current ? <Icon name="check" size={18} className="shrink-0 text-brand-text" /> : null}
      </Link>
      {current ? null : <AccessAction desktop={desktop} name={name} deviceId={deviceId} />}
    </li>
  );
}

/** The grant of this device on one desktop; read only while the sheet is open, requested only on tap. */
function AccessAction({ desktop, name, deviceId }: { desktop: DesktopSummary; name: string; deviceId: string | null }) {
  const queryClient = useQueryClient();
  const grants = useDesktopGrants(desktop.id);
  const action = useAction(desktopErrorMessage);
  if (!grants.data) return null;
  const mine = deviceId ? grants.data.grants.find((item) => item.deviceId === deviceId) : undefined;
  if (mine?.state === "pending") return <span className="px-2 text-meta text-muted-text">Waiting for confirmation on the desktop</span>;
  if (mine?.state === "revoked") return <span className="px-2 text-meta text-muted-text">Access revoked</span>;
  if (mine) return null;

  async function request() {
    await action.run(async () => {
      const identity = await ensureDevice();
      await requestGrantAndRefresh(queryClient, desktop.id, identity.deviceId);
      markGrantRequested(desktop.id, identity.deviceId);
    });
  }

  return (
    <div className="flex w-full flex-col gap-2 px-2 pb-2 sm:w-auto sm:pb-0">
      <Button type="button" variant="outline" size="sm" disabled={action.busy} aria-label={`Request access to ${name}`} onClick={() => void request()}>
        Request access
      </Button>
      {action.error ? <Notice variant="error">{action.error}</Notice> : null}
    </div>
  );
}
