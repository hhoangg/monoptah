// The activity rail (tablet and desktop, from 640 px): a vertical strip at the far left of the app shell, like the
// activity bar of an editor. Top: the account's desktops as avatars (initials, a color per desktop, an online dot, a bar
// beside the current one); a click opens that desktop. Bottom: Cloud (only where the desktop has it), Desktop settings
// and the account avatar, whose menu holds Account settings, All desktops and Install app. It replaces the top bar and
// its desktop switcher on these widths; the phone keeps the top bar (features/desktops/DesktopSwitcher.tsx).
import { Link } from "@tanstack/react-router";
import { useState, type KeyboardEvent, type ReactNode } from "react";
import { GRANT_CUE_LABEL, desktopAvatarColor, grantCue, onAvatarColor } from "@/features/desktops/avatar";
import { DesktopAvatarFace } from "@/features/desktops/DesktopAvatarFace";
import { useDesktopGrants, useDesktops, type DesktopSummary } from "@/features/desktops/api";
import { useThisDevice } from "@/features/desktops/thisDevice";
import { safeName } from "@/features/desktops/format";
import { CLOUD_NAV_LABEL } from "@/features/cloud/copy";
import { useLinkState } from "@/link";
import { cn } from "@/lib/utils";
import { AccountMenu } from "@/ui/AccountMenu";
import { Icon, type IconName } from "@/ui/Icon";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/ui/tooltip";
import { useCapability } from "@/version";

const button =
  "relative inline-flex size-11 shrink-0 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-10 [&.active]:bg-selection [&.active]:text-content";

interface RailDesktop {
  id: string;
  name: string;
  color: string | null;
  avatar: DesktopSummary["avatar"];
  online: boolean;
}

/** The desktops to list: the account's, with the open one filled in from the live link while the list has not loaded it. */
function useRailDesktops(desktopId: string): RailDesktop[] {
  const desktops = useDesktops();
  const linkName = useLinkState((state) => state.desktopName);
  const presence = useLinkState((state) => state.presence);
  const listed = desktops.data?.desktops ?? [];
  const items: RailDesktop[] = listed.map((item: DesktopSummary) => ({
    id: item.id,
    name: safeName(item.name, "Unnamed desktop"),
    color: item.color,
    avatar: item.avatar ?? null,
    // The live link knows better than the 15 s list for the open desktop.
    online: item.id === desktopId && presence !== null ? presence === "online" : item.online,
  }));
  if (!items.some((item) => item.id === desktopId)) {
    items.unshift({ id: desktopId, name: safeName(linkName ?? "", "Desktop"), color: null, avatar: null, online: presence === "online" });
  }
  return items;
}

function RailTip({ label, detail, children }: { label: string; detail?: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="right" className="hidden pointer-fine:block">
        <span className="block font-medium">{label}</span>
        {detail ? <span className="block text-muted-text">{detail}</span> : null}
      </TooltipContent>
    </Tooltip>
  );
}

function DesktopAvatar({ desktop, current, tabStop, onFocus }: { desktop: RailDesktop; current: boolean; tabStop: boolean; onFocus: () => void }) {
  const background = desktopAvatarColor(desktop.id, desktop.color);
  const status = desktop.online ? "Online" : "Offline";
  // The grant of this device, as the old switcher sheet showed it. The open desktop is connected, so it has access.
  const grants = useDesktopGrants(desktop.id);
  const thisDevice = useThisDevice();
  const cue = current ? null : grantCue(grants.data?.grants, thisDevice?.deviceId ?? null);
  const cueText = cue ? GRANT_CUE_LABEL[cue] : null;
  return (
    <li className="relative flex w-full justify-center">
      {current ? <span aria-hidden="true" data-testid="rail-indicator" className="absolute top-1/2 left-0 h-6 w-[3px] -translate-y-1/2 rounded-r-pill bg-content" /> : null}
      <RailTip label={desktop.name} detail={cueText ? `${status}, ${cueText}` : status}>
        <Link
          to="/d/$desktopId"
          params={{ desktopId: desktop.id }}
          data-rail-desktop=""
          data-grant={cue ?? undefined}
          tabIndex={tabStop ? 0 : -1}
          onFocus={onFocus}
          aria-current={current ? "page" : undefined}
          aria-label={`${desktop.name}, ${status.toLowerCase()}${cueText ? `, ${cueText}` : ""}${current ? ", current desktop" : ""}`}
          className={cn(
            "relative inline-flex size-11 shrink-0 items-center justify-center text-small font-semibold outline-offset-2 hover:brightness-110 motion-safe:transition-[border-radius] motion-safe:duration-150 pointer-fine:size-10",
            // The current desktop squares off a little (and has the bar beside it); the others stay round.
            current ? "rounded-row" : "rounded-pill",
          )}
          style={{ backgroundColor: background, color: onAvatarColor(background) }}
        >
          <DesktopAvatarFace desktop={desktop} />
          <span
            aria-hidden="true"
            data-status={status.toLowerCase()}
            className={cn("absolute -right-0.5 -bottom-0.5 size-3 rounded-pill border-2 border-background", desktop.online ? "bg-additions" : "bg-faint")}
          />
          {cue ? (
            <span aria-hidden="true" data-testid="rail-lock" className="absolute -top-1 -right-1 inline-flex size-4 items-center justify-center rounded-pill border border-stroke bg-sheet text-content">
              <Icon name="lock" size={10} />
            </span>
          ) : null}
        </Link>
      </RailTip>
    </li>
  );
}

function RailLink({ to, desktopId, label, icon }: { to: "/d/$desktopId/cloud" | "/d/$desktopId/settings"; desktopId: string; label: string; icon: IconName }) {
  return (
    <li className="flex w-full justify-center">
      <RailTip label={label}>
        <Link to={to} params={{ desktopId }} aria-label={label} className={button}>
          <Icon name={icon} />
        </Link>
      </RailTip>
    </li>
  );
}

export function ActivityRail({ desktopId }: { desktopId: string }) {
  const desktops = useRailDesktops(desktopId);
  const cloud = useCapability("cloud");

  // Roving tabindex: the desktop list is one tab stop (the last focused desktop, else the open one); Arrow Up/Down,
  // Home and End move inside it.
  const [roving, setRoving] = useState<string | null>(null);
  const stop = roving !== null && desktops.some((item) => item.id === roving) ? roving : desktopId;
  function onKeyDown(event: KeyboardEvent<HTMLUListElement>) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const links = Array.from(event.currentTarget.querySelectorAll<HTMLAnchorElement>("a[data-rail-desktop]"));
    const at = links.indexOf(document.activeElement as HTMLAnchorElement);
    if (at < 0) return;
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? links.length - 1 : (at + (event.key === "ArrowDown" ? 1 : -1) + links.length) % links.length;
    links[next]?.focus();
  }

  return (
    <TooltipProvider delayDuration={300}>
      <nav
        aria-label="Desktops and account"
        data-testid="activity-rail"
        className="flex h-full min-h-0 w-14 shrink-0 flex-col items-center border-r border-stroke bg-background pb-[max(0.5rem,env(safe-area-inset-bottom))]"
      >
        <ul
          aria-label="Desktops"
          data-testid="rail-desktops"
          onKeyDown={onKeyDown}
          className="m-0 flex min-h-0 w-full flex-1 list-none flex-col items-center gap-3 overflow-y-auto overscroll-contain p-0 py-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        >
          {desktops.map((desktop) => (
            <DesktopAvatar key={desktop.id} desktop={desktop} current={desktop.id === desktopId} tabStop={desktop.id === stop} onFocus={() => setRoving(desktop.id)} />
          ))}
        </ul>
        <ul data-testid="rail-actions" className="m-0 flex w-full list-none flex-col items-center gap-1 border-t border-stroke p-0 pt-2">
          {cloud ? <RailLink to="/d/$desktopId/cloud" desktopId={desktopId} label={CLOUD_NAV_LABEL} icon="cloud" /> : null}
          <RailLink to="/d/$desktopId/settings" desktopId={desktopId} label="Desktop settings" icon="settings" />
          <li className="flex w-full justify-center">
            <AccountMenu side="right" />
          </li>
        </ul>
      </nav>
    </TooltipProvider>
  );
}
