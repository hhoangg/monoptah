// Layout of /d/:desktopId: mounts the LinkProvider and the Wave 0 slots. Owner: WS-L.
import { createFileRoute, Link, Outlet, useNavigate, useRouterState } from "@tanstack/react-router";
import { useEffect, useRef } from "react";
import { useLink } from "@/link/store";
import { useLinkDataSync } from "@/data/sync";
import { useDesktops } from "@/features/desktops/api";
import { rememberDesktop } from "@/features/sessions/lastPlace";
import { LinkProvider, useLinkState } from "@/link/LinkProvider";
import { buttonVariants } from "@/ui/button";
import { Card, CardDescription } from "@/ui/card";
import { Slot } from "@/ui/slots";
import { LinkBlockedScreen, isVersionBlocked } from "@/version/LinkBlockedScreen";

export const Route = createFileRoute("/d/$desktopId")({
  component: DesktopLayout,
});

function DesktopLayout() {
  const { desktopId } = Route.useParams();
  return (
    <LinkProvider desktopId={desktopId}>
      <DesktopGate desktopId={desktopId} />
    </LinkProvider>
  );
}

/** B-6: a desktop that is not in the account's list gets a way out instead of the workspace; everything else goes on to the guards. */
function DesktopGate({ desktopId }: { desktopId: string }) {
  const desktops = useDesktops();
  // The list loaded without this desktop (revoked or removed) and the link was turned away. A ready link wins, so a
  // stale cached list cannot block a desktop that was just paired.
  const linkReady = useLink(desktopId, (state) => state.status === "ready");
  // Only a link the server or desktop turned away counts: a link still connecting, or blocked by a version mismatch or a
  // login redirect, keeps its own screens even when the list does not know the id.
  // The real revoke path ends with a closed link and a directory 404/403 (the directory no longer knows the desktop).
  const refused = useLink(
    desktopId,
    (state) =>
      state.blocked === "revoked" ||
      state.blocked === "desktop_refused" ||
      (state.status === "closed" && state.blocked === null && (state.lastError?.code === "directory_404" || state.lastError?.code === "directory_403")),
  );
  const gone = desktops.isSuccess && refused && !linkReady && !desktops.data.desktops.some((desktop) => desktop.id === desktopId);
  // N-12: any screen of a desktop makes it the one `/` goes back to (never one that is gone).
  useEffect(() => (gone ? undefined : rememberDesktop(desktopId)), [desktopId, gone]);
  return gone ? <DesktopGoneScreen /> : <LinkGuards />;
}

/** Link-level guards: login redirect and the W-22 version screen. The slots below are Wave 0 and unchanged. */
function LinkGuards() {
  // Catalog polling, resync and the clear-on-revoked run for as long as the layout is mounted (ref counted).
  useLinkDataSync();
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

/** B-6: a desktop that is no longer in the account has nothing to load; say so and point to the way forward. */
function DesktopGoneScreen() {
  return (
    <div data-testid="desktop-gone-screen" className="mx-auto flex w-full max-w-[480px] flex-col gap-4 px-5 py-10">
      <Card className="flex flex-col gap-3 p-5">
        <h1 className="text-body font-semibold">This desktop is no longer in your account</h1>
        <CardDescription>It was revoked or removed. Pair it again to use it from the web.</CardDescription>
        <div className="flex flex-col gap-2">
          <Link to="/desktops" className={buttonVariants({ size: "lg", className: "w-full" })}>
            Go to Desktops
          </Link>
          <Link to="/desktops" className={buttonVariants({ variant: "secondary", size: "lg", className: "w-full" })}>
            Pair a desktop
          </Link>
        </div>
      </Card>
    </div>
  );
}
