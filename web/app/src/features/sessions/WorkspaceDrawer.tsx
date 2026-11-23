// The workspace list as a left vaul drawer, opened from a 44 px button at the left of the /d top bar below 640 px
// (from 640 px the app shell's sidebar carries the list). The button says when a Mono has a reply not seen on this device.
import { useRouterState } from "@tanstack/react-router";
import { useState } from "react";
import { Drawer as DrawerPrimitive } from "vaul";
import { DrawerOverlay, DrawerPortal, DrawerTitle } from "@/ui/drawer";
import { Icon } from "@/ui/Icon";
import { useCatalogUnreadMonos } from "./monoSeen";
import { WorkspaceNav } from "./WorkspaceNav";

export function WorkspaceDrawerToggle({ desktopId }: { desktopId: string }) {
  // The drawer is open for one location only: any navigation that does not go through the nav rows (the Cloud entry of a
  // slot, history back or forward, a deep link) changes the pathname and so closes it, without an effect.
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const [openPath, setOpenPath] = useState<string | null>(null);
  // Drop a stale value while rendering, so coming back to the same path (history back) does not reopen the drawer.
  if (openPath !== null && openPath !== pathname) setOpenPath(null);
  const open = openPath === pathname;
  // F-17: the menu button says when a Mono has a reply not seen on this device.
  const hasUnread = useCatalogUnreadMonos(desktopId).size > 0;
  return (
    <DrawerPrimitive.Root direction="left" open={open} onOpenChange={(next) => setOpenPath(next ? pathname : null)}>
      <DrawerPrimitive.Trigger asChild>
        <button
          type="button"
          aria-label={hasUnread ? "Open workspace list, new replies" : "Open workspace list"}
          data-unread={hasUnread}
          className="relative inline-flex size-11 shrink-0 items-center justify-center rounded-pill hover:bg-selection"
        >
          <Icon name="sidebar" size={20} />
          {hasUnread ? <span aria-hidden="true" data-testid="drawer-unread-badge" className="absolute top-2 right-2 size-2.5 rounded-pill border-2 border-background bg-brand" /> : null}
        </button>
      </DrawerPrimitive.Trigger>
      <DrawerPortal>
        <DrawerOverlay />
        <DrawerPrimitive.Content
          data-slot="workspace-drawer"
          aria-describedby={undefined}
          className="fixed inset-y-0 left-0 z-50 flex w-[min(82vw,330px)] flex-col border-r border-stroke bg-drawer pt-[env(safe-area-inset-top)] pb-[env(safe-area-inset-bottom)] text-content"
        >
          <DrawerTitle className="sr-only">Workspace</DrawerTitle>
          <WorkspaceNav desktopId={desktopId} onNavigate={() => setOpenPath(null)} />
        </DrawerPrimitive.Content>
      </DrawerPortal>
    </DrawerPrimitive.Root>
  );
}
