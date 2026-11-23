// One rule for the app shell's footer bar: from 640 px it shows on every screen; on a phone only on the workspace home
// (`/d/:desktopId`). The shell uses it to render the bar, and WorkspaceFrame to know who clears the bottom safe-area inset.
import { useRouterState } from "@tanstack/react-router";
import { useMediaQuery } from "@/ui/useMediaQuery";

const FOOTER_WIDE_QUERY = "(min-width: 640px)";

export function useFooterShown(desktopId: string): boolean {
  const wide = useMediaQuery(FOOTER_WIDE_QUERY);
  const workspaceHome = useRouterState({ select: (state) => state.location.pathname.replace(/\/$/, "") === `/d/${desktopId}` });
  return wide || workspaceHome;
}
