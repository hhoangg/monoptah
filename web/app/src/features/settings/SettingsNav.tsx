// The nav of the settings shell: a quick-find box over the static settings index, then the grouped page links (Account,
// This browser, Desktops with one item per desktop, Admin for admins). It is the left column from 1024px and the list
// page below that (see SettingsShell).
import { Link, useRouter, type LinkProps } from "@tanstack/react-router";
import { useId, useState, type ReactNode } from "react";
import { desktopAvatarColor, onAvatarColor } from "@/features/desktops/avatar";
import { DesktopAvatarFace } from "@/features/desktops/DesktopAvatarFace";
import { useDesktops, type DesktopSummary } from "@/features/desktops/api";
import { safeName } from "@/features/desktops/format";
import { cn } from "@/lib/utils";
import { Badge } from "@/ui/badge";
import { Icon, type IconName } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { MatchText } from "@/ui/MatchText";
import { SETTINGS_HIDE_DIVIDER_SELF_WRAPPED, SETTINGS_NAV_CARD_BODY, SETTINGS_NAV_DIVIDER_ICON, SETTINGS_NAV_DIVIDER_PLAIN } from "@/ui/SettingsCard";
import { requestScrollIfAlreadyThere } from "./scrollRequest";
import { SettingsExitButton } from "./settingsExit";
import { BROWSER_ONLY_LABEL, SETTINGS_GROUP_LABELS, searchSettings, type SettingsIndexEntry } from "./settingsIndex";

// One look, two layouts by CSS only. From lg: a compact sidebar row (min-h-11, min-h-10 with a mouse, active bg-fill) with
// the same room left and right of the highlight as above and below the 24px icon (10px with min-h-11, 8px with min-h-10). Below lg: the list page, an
// iOS-style row of a card (min-h-13, divider from the text start, chevron), like SettingsRow; the shared classes of
// ui/SettingsCard.tsx give it the card's equal inset and concentric 12px rows.
const ITEM_CLASS =
  "flex min-h-13 items-center gap-x-3 rounded-row px-1.5 text-small font-medium text-muted-text hover:bg-selection max-lg:text-row max-lg:text-content lg:min-h-11 lg:gap-x-2.5 lg:px-2.5 lg:pointer-fine:min-h-10 lg:pointer-fine:px-2 aria-[current=page]:bg-fill aria-[current=page]:text-content";
const LIST_CLASS = `m-0 flex list-none flex-col p-0 lg:gap-0.5 ${SETTINGS_NAV_CARD_BODY}`;
// The divider starts after the icon slot (like SettingsRow); a search result has no icon, so it starts at the text inset.
// It fades out around the highlighted (hovered, focused or current) row, which is the link inside the `<li>`.
const LI_ICON_CLASS = `relative ${SETTINGS_NAV_DIVIDER_ICON} ${SETTINGS_HIDE_DIVIDER_SELF_WRAPPED}`;
const LI_PLAIN_CLASS = `relative ${SETTINGS_NAV_DIVIDER_PLAIN} ${SETTINGS_HIDE_DIVIDER_SELF_WRAPPED}`;

/** The 36px icon slot of a row (24px from lg), holding an icon or a desktop avatar. */
function IconSlot({ children }: { children: ReactNode }) {
  return <span className="inline-flex size-9 shrink-0 items-center justify-center text-muted-text lg:size-6">{children}</span>;
}

/** The chevron of the list page rows; hidden from lg, where the nav is a plain sidebar. */
function Chevron() {
  return <Icon name="chevronRight" size={18} className="ml-auto shrink-0 text-faint lg:hidden" />;
}

function NavGroup({ title, badge, children }: { title: string; badge?: string; children: ReactNode }) {
  // A plain label, not a heading: group labels such as "Account" would otherwise join the page's heading outline.
  const labelId = useId();
  return (
    <div className="flex flex-col gap-1">
      <p id={labelId} className="m-0 flex flex-wrap items-center gap-2 px-3 pb-1 text-meta font-medium text-muted-text">
        {title}
        {badge ? <Badge variant="outline">{badge}</Badge> : null}
      </p>
      <ul aria-labelledby={labelId} className={LIST_CLASS}>
        {children}
      </ul>
    </div>
  );
}

function NavItem({ to, icon, exact, children }: { to: LinkProps["to"]; icon: IconName; exact?: boolean; children: ReactNode }) {
  return (
    <li className={LI_ICON_CLASS}>
      <Link to={to} activeOptions={{ exact: exact ?? false }} data-row-highlight="" className={ITEM_CLASS}>
        <IconSlot>
          <Icon name={icon} size={18} />
        </IconSlot>
        {children}
        <Chevron />
      </Link>
    </li>
  );
}

/** A small round desktop avatar with its presence dot; the accessible state is in the link name, not the dot. */
function NavAvatar({ desktop }: { desktop: DesktopSummary }) {
  const background = desktopAvatarColor(desktop.id, desktop.color);
  return (
    <span
      aria-hidden="true"
      data-testid="nav-avatar"
      className="relative inline-flex size-6 shrink-0 items-center justify-center rounded-pill text-caption font-semibold"
      style={{ backgroundColor: background, color: onAvatarColor(background) }}
    >
      <DesktopAvatarFace desktop={desktop} emojiClass="text-sm" />
      <span className={cn("absolute -right-0.5 -bottom-0.5 size-2.5 rounded-pill border-2 border-background", desktop.online ? "bg-additions" : "bg-faint")} />
    </span>
  );
}

interface ResultRow {
  key: string;
  entry: SettingsIndexEntry;
  positions: number[];
  /** The desktop a per-desktop entry was expanded for. */
  desktop?: DesktopSummary;
}

function resultRows(query: string, admin: boolean, desktops: readonly DesktopSummary[]): ResultRow[] {
  const rows: ResultRow[] = [];
  for (const { entry, positions } of searchSettings(query, { admin })) {
    if (entry.perDesktop) {
      for (const desktop of desktops) rows.push({ key: `${entry.id}:${desktop.id}`, entry, positions, desktop });
    } else {
      rows.push({ key: entry.id, entry, positions });
    }
  }
  return rows;
}

function ResultLink({ row, onPick }: { row: ResultRow; onPick: () => void }) {
  const { entry, desktop } = row;
  const router = useRouter();
  const detail = desktop ? safeName(desktop.name, "Unnamed desktop") : entry.label === entry.page ? undefined : entry.page;
  const body = (
    <span className="min-w-0">
      <span className="block">
        <MatchText text={entry.label} positions={row.positions} active />
      </span>
      {detail ? <span className="block text-meta font-normal text-muted-text">{detail}</span> : null}
    </span>
  );
  const className = cn(ITEM_CLASS, "py-2");
  const hash = entry.anchor;
  const pick = (pathname: string) => {
    requestScrollIfAlreadyThere(router.state.location, { pathname, hash });
    onPick();
  };
  if (desktop) {
    return (
      <Link to="/settings/desktops/$id" params={{ id: desktop.id }} hash={hash} onClick={() => pick(`/settings/desktops/${desktop.id}`)} data-row-highlight="" className={className}>
        {body}
      </Link>
    );
  }
  const route = entry.route;
  if (route === "/settings/desktops/$id") return null;
  return (
    <Link to={route} hash={hash} onClick={() => pick(route)} data-row-highlight="" className={className}>
      {body}
    </Link>
  );
}

export function SettingsNav({ admin }: { admin: boolean }) {
  const [query, setQuery] = useState("");
  const inputId = useId();
  const desktops = useDesktops().data?.desktops ?? [];
  const searching = query.trim() !== "";
  const rows = searching ? resultRows(query, admin, desktops) : [];
  const clear = () => setQuery("");
  return (
    <nav aria-label="Settings" className="flex flex-col gap-4">
      {/* The wide column's title row (the narrow list page has its own header with the same button). Not a heading: the page keeps its single h1. */}
      <div className="hidden items-center gap-2 lg:flex">
        <SettingsExitButton />
        <span className="text-headline font-semibold text-content">Settings</span>
      </div>
      <Input
        id={inputId}
        type="search"
        aria-label="Find a setting"
        placeholder="Find a setting"
        autoComplete="off"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape" && query !== "") {
            event.preventDefault();
            clear();
          }
        }}
      />
      {searching ? (
        rows.length === 0 ? (
          <p role="status" className="px-3 text-small text-muted-text">
            No settings match.
          </p>
        ) : (
          <ul aria-label="Matching settings" className={LIST_CLASS}>
            {rows.map((row) => (
              <li key={row.key} className={LI_PLAIN_CLASS}>
                <ResultLink row={row} onPick={clear} />
              </li>
            ))}
          </ul>
        )
      ) : (
        <>
          <NavGroup title={SETTINGS_GROUP_LABELS.account}>
            <NavItem to="/settings/profile" icon="account">Profile</NavItem>
            <NavItem to="/settings/security" icon="lock">Security</NavItem>
            <NavItem to="/settings/invites" icon="userAdd">Invites</NavItem>
          </NavGroup>
          <NavGroup title={SETTINGS_GROUP_LABELS.browser} badge={BROWSER_ONLY_LABEL}>
            <NavItem to="/settings/appearance" icon="appearance">Appearance</NavItem>
            <NavItem to="/settings/notifications" icon="notifications">Notifications</NavItem>
            <NavItem to="/settings/install" icon="download">Install app</NavItem>
          </NavGroup>
          <NavGroup title={SETTINGS_GROUP_LABELS.desktops}>
            <NavItem to="/settings/desktops" icon="computer" exact>
              All desktops
            </NavItem>
            {desktops.map((desktop) => (
              <li key={desktop.id} className={cn(LI_ICON_CLASS, "lg:ml-4")}>
                <Link to="/settings/desktops/$id" params={{ id: desktop.id }} data-row-highlight="" className={ITEM_CLASS}>
                  <IconSlot>
                    <NavAvatar desktop={desktop} />
                  </IconSlot>
                  <span className="min-w-0 truncate">{safeName(desktop.name, "Unnamed desktop")}</span>
                  <span className="sr-only">{desktop.online ? ", online" : ", offline"}</span>
                  <Chevron />
                </Link>
              </li>
            ))}
          </NavGroup>
          {admin ? (
            <NavGroup title={SETTINGS_GROUP_LABELS.admin}>
              <NavItem to="/settings/admin" icon="admin" exact>
                Overview
              </NavItem>
              <NavItem to="/settings/admin/users" icon="people">Users</NavItem>
              <NavItem to="/settings/admin/invites" icon="link">Invite tree</NavItem>
            </NavGroup>
          ) : null}
        </>
      )}
    </nav>
  );
}
