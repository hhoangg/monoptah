// The five named slots WS-T hosts in the session screens (workstreams.md 3.2, "Web slots in the session
// screens"). Each host is an empty component by default: it renders the component WS-C registered with
// `registerSlot(name, Component)` from `features/cloud/register.ts`, or nothing. `fallback` is what the
// host's own screen shows when no one registered (only `archive-dialog` needs one: W-08's keep-or-stop
// dialog for cloud sessions replaces the plain archive confirmation).
import type { ReactNode } from "react";
import type { SessionCard } from "@monoptah/protocol";
import { getSlot, Slot } from "@/ui/slots";

interface HostProps {
  desktopId: string;
  /** What to show when nothing is registered for the slot. */
  fallback?: ReactNode;
}

/** Above the composer: the approval card and the cloud banners (W-08, filled by WS-C). */
export function ComposerTopSlot({ desktopId, sessionId, fallback = null }: HostProps & { sessionId: string }) {
  if (!getSlot("composer-top")) return <>{fallback}</>;
  return <Slot name="composer-top" desktopId={desktopId} sessionId={sessionId} />;
}

/** On a session card in the list: the target badge (W-06, filled by WS-C). */
export function SessionCardBadgeSlot({ desktopId, session, fallback = null }: HostProps & { session: SessionCard }) {
  if (!getSlot("session-card-badge")) return <>{fallback}</>;
  return <Slot name="session-card-badge" desktopId={desktopId} session={session} />;
}

/** In the session header: the cloud state chip (W-08, filled by WS-C). */
export function SessionHeaderChipSlot({ desktopId, session, fallback = null }: HostProps & { session: SessionCard }) {
  if (!getSlot("session-header-chip")) return <>{fallback}</>;
  return <Slot name="session-header-chip" desktopId={desktopId} session={session} />;
}

/** Extra items of the session menus (W-06 card menu, W-08 header menu). Render inside a menu content. */
export function SessionMenuItemsSlot({ desktopId, session, fallback = null }: HostProps & { session: SessionCard }) {
  if (!getSlot("session-menu-items")) return <>{fallback}</>;
  return <Slot name="session-menu-items" desktopId={desktopId} session={session} />;
}

/** The archive confirmation (W-08); WS-C replaces it with the keep-or-stop dialog for cloud sessions. */
export function ArchiveDialogSlot({
  desktopId,
  session,
  open,
  onOpenChange,
  fallback = null,
}: HostProps & { session: SessionCard; open: boolean; onOpenChange: (open: boolean) => void }) {
  if (!getSlot("archive-dialog")) return <>{fallback}</>;
  return <Slot name="archive-dialog" desktopId={desktopId} session={session} open={open} onOpenChange={onOpenChange} />;
}
