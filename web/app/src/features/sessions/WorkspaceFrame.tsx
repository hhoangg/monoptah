// The frame of the WS-T screens (home and project): the workspace list as a vaul drawer opened from the header
// below 640 px (from 640 px the app shell's sidebar carries it), a header with the screen's only h1,
// and the shared effects of a workspace screen (remember the place, track the working sessions).
import { Link, type LinkProps } from "@tanstack/react-router";
import { useEffect, useState, type ReactNode } from "react";
import { Drawer as DrawerPrimitive } from "vaul";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { DrawerOverlay, DrawerPortal, DrawerTitle } from "@/ui/drawer";
import { cn } from "@/lib/utils";
import { Icon } from "@/ui/Icon";
import { trackSessions } from "./activityStore";
import { useRememberPlace } from "./lastPlace";
import { RequestControlChip } from "./RequestControlChip";
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
  /** A 44 px back link before the title (a project tool goes back to its project). */
  back?: { to: LinkProps["to"]; params?: LinkProps["params"]; label: string };
  /** A full-height screen: fills the pane and scrolls inside, with no bottom padding. */
  fill?: boolean;
  /** A normal screen of one column: capped at 760 px (design-system 11) instead of the full pane. */
  narrow?: boolean;
}

export function WorkspaceFrame({ desktopId, title, subtitle, actions, children, testId = "workspace-frame", back, fill = false, narrow = false }: WorkspaceFrameProps) {
  useRememberPlace();
  useActivityTracking(desktopId);
  const readOnly = useReadOnly();
  const wide = useMediaQuery(WIDE_QUERY);
  const [open, setOpen] = useState(false);
  // F-17: the menu button says when a Mono has a reply not seen on this device.
  const hasUnread = useCatalogUnreadMonos(desktopId).size > 0;

  return (
    <div className={cn("mx-auto flex w-full", narrow ? "max-w-[760px]" : "max-w-[1280px]", fill && "h-full min-h-0 flex-1")} data-testid={testId}>
      <div className={cn("min-w-0 flex-1 px-4 pt-2 min-[24rem]:px-5", fill ? "flex min-h-0 flex-col pb-2" : "pb-40")}>
        <header className="flex shrink-0 items-start gap-3 pb-3">
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
                  className="fixed inset-y-0 left-0 z-50 flex w-[min(82vw,330px)] flex-col border-r border-stroke bg-drawer pt-[env(safe-area-inset-top)] pb-[env(safe-area-inset-bottom)] text-content"
                >
                  <DrawerTitle className="sr-only">Workspace</DrawerTitle>
                  <WorkspaceNav desktopId={desktopId} onNavigate={() => setOpen(false)} />
                </DrawerPrimitive.Content>
              </DrawerPortal>
            </DrawerPrimitive.Root>
          )}
          {back ? (
            <Link to={back.to} params={back.params} aria-label={back.label} className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection">
              <Icon name="chevronDown" size={20} className="rotate-90" />
            </Link>
          ) : null}
          <div className="min-w-0 flex-1">
            <h1 className="wrap-anywhere text-headline font-semibold">{title}</h1>
            {subtitle ? (
              // A path loses its start, not the folder that tells projects apart: the ellipsis goes on the left.
              <p className="truncate text-left text-small text-muted-text" dir={typeof subtitle === "string" && /^[/~]/.test(subtitle) ? "rtl" : undefined} title={typeof subtitle === "string" ? subtitle : undefined}>
                <bdi>{subtitle}</bdi>
              </p>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {readOnly ? <RequestControlChip desktopId={desktopId} /> : null}
            {actions}
          </div>
        </header>
        {children}
      </div>
    </div>
  );
}
