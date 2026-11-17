// Layout of /d/:desktopId: mounts the LinkProvider and the Wave 0 slots. Owner: WS-L.
import { createFileRoute, Outlet, useNavigate, useRouterState } from "@tanstack/react-router";
import { useEffect, useRef } from "react";
import { LinkProvider, useLinkState } from "@/link/LinkProvider";
import { Slot } from "@/ui/slots";
import { LinkBlockedScreen, isVersionBlocked } from "@/version/LinkBlockedScreen";

export const Route = createFileRoute("/d/$desktopId")({
  component: DesktopLayout,
});

function DesktopLayout() {
  const { desktopId } = Route.useParams();
  return (
    <LinkProvider desktopId={desktopId}>
      <LinkGuards />
    </LinkProvider>
  );
}

/** Link-level guards: login redirect and the W-22 version screen. The slots below are Wave 0 and unchanged. */
function LinkGuards() {
  const blocked = useLinkState((state) => state.blocked);
  const navigate = useNavigate();
  const href = useRouterState({ select: (state) => state.location.href });
  // Once per block: the layout stays mounted for a render or two while the router already shows /login.
  const redirected = useRef(false);

  useEffect(() => {
    if (blocked !== "login_required") {
      redirected.current = false;
      return;
    }
    if (redirected.current) return;
    redirected.current = true;
    void navigate({ to: "/login", search: { redirect: href } as never, replace: true });
  }, [blocked, href, navigate]);

  if (isVersionBlocked(blocked)) return <LinkBlockedScreen reason={blocked} />;
  return (
    <>
      <Slot name="desktop-switcher" />
      <Slot name="link-state-screens" />
      <Slot name="first-run-picker" />
      <Slot name="app-shell">
        <Outlet />
      </Slot>
    </>
  );
}
