// The frame of the WS-T screens (home and project): a header with the screen's only h1 (the workspace list drawer
// below 640 px lives in the /d top bar, see WorkspaceDrawer.tsx),
// and the shared effects of a workspace screen (remember the place, track the working sessions).
import { Link, type LinkProps } from "@tanstack/react-router";
import { useEffect, type ReactNode } from "react";
import { useCatalog } from "@/data";
import { useLinkState } from "@/link";
import { cn } from "@/lib/utils";
import { Icon } from "@/ui/Icon";
import { trackSessions } from "./activityStore";
import { useRememberPlace } from "./lastPlace";
import { useFooterShown } from "./useFooterShown";
import { RequestControlChip } from "./RequestControlChip";

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
  /** No bottom padding: the screen ends with its own bottom-anchored bar (the project page's now-working pill). */
  flush?: boolean;
  /** A one-line title (truncated, full name on hover) instead of wrapping, for headers that also carry a primary action. */
  compact?: boolean;
  /** The title stays as the screen's h1 for assistive technology but is not shown (the top bar already says where you are). */
  hideTitle?: boolean;
  /** Decorative element before the title (the project avatar). */
  leading?: ReactNode;
}

export function WorkspaceFrame({ desktopId, title, subtitle, actions, children, testId = "workspace-frame", back, fill = false, narrow = false, flush = false, compact = false, hideTitle = false, leading }: WorkspaceFrameProps) {
  useRememberPlace();
  useActivityTracking(desktopId);
  const readOnly = useReadOnly();
  // The app shell's footer bar (which carries the bottom safe-area inset) shows from 640 px and, on a phone, only on the
  // workspace home. Everywhere else the page ends at the screen edge and clears the inset itself.
  const footerShown = useFooterShown(desktopId);
  const showHeader = !hideTitle || !!(back || subtitle || actions || readOnly || leading);

  return (
    <div className={cn("mx-auto flex w-full", narrow ? "max-w-[760px]" : "max-w-[1280px]", fill ? "h-full min-h-0 flex-1" : "flex-1")} data-testid={testId}>
      <div className={cn("min-w-0 flex-1 px-4 pt-2 min-[24rem]:px-5", fill ? "flex min-h-0 flex-col pb-2" : flush ? "pb-0" : footerShown ? "pb-10" : "pb-[calc(2.5rem+env(safe-area-inset-bottom))]")}>
        {hideTitle ? <h1 className="sr-only">{title}</h1> : null}
        {showHeader ? (
          <header className={cn("flex shrink-0 gap-3 pb-3", compact ? "items-center" : "items-start")}>
            {back ? (
              <Link to={back.to} params={back.params} aria-label={back.label} className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill hover:bg-selection">
                <Icon name="chevronDown" size={20} className="rotate-90" />
              </Link>
            ) : null}
            {leading}
            <div className="min-w-0 flex-1">
              {hideTitle ? null : (
                <h1 className={cn("text-headline font-semibold", compact ? "truncate" : "wrap-anywhere")} title={compact ? title : undefined}>
                  {title}
                </h1>
              )}
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
        ) : null}
        {children}
      </div>
    </div>
  );
}
