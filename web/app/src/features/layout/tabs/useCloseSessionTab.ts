// The tab of a session that was deleted or archived goes with it: it leaves every pane of the desktop's workspace, and
// when the URL still points at the session the page moves on to the project page (the project's New session page, not
// the pane's next tab: the pane shows that page in place of its active tab until a tab is chosen).
import { useRouter } from "@tanstack/react-router";
import { useCallback } from "react";
import { pruneTabs, sessionTab } from "./paneTree";
import { tabOfPath } from "./tabRoutes";
import { updateWorkspace } from "./tabsStore";

export function useCloseSessionTab(desktopId: string): (session: { id: string; projectId: string }) => void {
  const router = useRouter();
  return useCallback(
    (session) => {
      const key = sessionTab(session.id);
      updateWorkspace(desktopId, (ws) => pruneTabs(ws, (tab) => tab !== key));
      if (tabOfPath(router.state.location.pathname) === key) {
        void router.navigate({ to: "/d/$desktopId/p/$projectId", params: { desktopId, projectId: session.projectId } });
      }
    },
    [desktopId, router],
  );
}
