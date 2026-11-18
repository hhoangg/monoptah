// The frame of the WS-T screens (home and project): the workspace list as a persistent sidebar from
// 640 px up and as a vaul drawer opened from the header below that, a header with the screen's only h1,
// and the shared effects of a workspace screen (remember the place, track the working sessions).
import { useEffect, useState, type ReactNode } from "react";
import { Drawer as DrawerPrimitive } from "vaul";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { Badge } from "@/ui/badge";
import { DrawerOverlay, DrawerPortal, DrawerTitle } from "@/ui/drawer";
import { Glass } from "@/ui/Glass";
import { Icon } from "@/ui/Icon";
import { trackSessions } from "./activityStore";
import { useRememberPlace } from "./lastPlace";
import { useCatalogUnreadMonos } from "./monoSeen";
import { WorkspaceNav } from "./WorkspaceNav";

/** True when the device may only read (`permission: read`). */
export function useReadOnly(): boolean {
  return useLinkState((state) => state.permission === "read");
}

/** Feeds the catalog's sessions to the activity tracker (F-27) for as long as a workspace screen is up. */
export function useActivityTracking(desktopId: string): void {
  const sessions = useCatalog().data?.sessions;
  useEffect(() => {
    if (sessions) trackSessions(desktopId, sessions);
  }, [desktopId, sessions]);
}

export const WIDE_QUERY = "(min-width: 640px)";

export interface WorkspaceFrameProps {
  desktopId: string;
  title: string;
  /** Under the title, for example the project path. */
  subtitle?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  /** Overrides the root test id. */
  testId?: string;
}

export function WorkspaceFrame({ desktopId, title, subtitle, actions, children, testId = "workspace-frame" }: WorkspaceFrameProps) {
  useRememberPlace();
  useActivityTracking(desktopId);
  const readOnly = useReadOnly();
  const wide = useMediaQuery(WIDE_QUERY);
  const [open, setOpen] = useState(false);
  // F-17: the menu button says when a Mono has a reply not seen on this device.
  const hasUnread = useCatalogUnreadMonos(desktopId).size > 0;

  return (
    <div className="mx-auto flex w-full max-w-[1280px]" data-testid={testId}>
      {wide ? (
        <aside className="sticky top-[calc(2.75rem+env(safe-area-inset-top))] h-[calc(100dvh-2.75rem-env(safe-area-inset-top))] w-80 shrink-0 border-r border-stroke">
          <WorkspaceNav desktopId={desktopId} />
        </aside>
      ) : null}
      <div className="min-w-0 flex-1 px-4 pt-2 pb-40 min-[24rem]:px-5">
        <header className="flex items-start gap-3 pb-3">
          {wide ? null : (
            <DrawerPrimitive.Root direction="left" open={open} onOpenChange={setOpen}>
              <DrawerPrimitive.Trigger asChild>
                <button
                  type="button"
                  aria-label={hasUnread ? "Open workspace list, new replies" : "Open workspace list"}
                  data-unread={hasUnread}
                  className="relative inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection"
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
                  className="fixed inset-y-0 left-0 z-50 flex w-[min(88vw,360px)] flex-col border-r border-stroke bg-drawer pt-[env(safe-area-inset-top)] pb-[env(safe-area-inset-bottom)] text-content"
                >
                  <DrawerTitle className="sr-only">Workspace</DrawerTitle>
                  <WorkspaceNav desktopId={desktopId} onNavigate={() => setOpen(false)} />
                </DrawerPrimitive.Content>
              </DrawerPortal>
            </DrawerPrimitive.Root>
          )}
          <div className="min-w-0 flex-1">
            <h1 className="wrap-anywhere text-headline font-semibold">{title}</h1>
            {subtitle ? <p className="truncate text-small text-muted-text">{subtitle}</p> : null}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {readOnly ? (
              <Glass className="flex min-h-9 items-center rounded-pill px-3">
                <Badge variant="outline" className="border-0 px-0 text-meta" data-testid="read-only-chip">
                  Read only
                </Badge>
              </Glass>
            ) : null}
            {actions}
          </div>
        </header>
        {children}
      </div>
    </div>
  );
}
