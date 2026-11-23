// The `link-state-screens` slot of `/d/:desktopId` (W-04, N-08). Owner: WS-M.
//
// The layout renders slots NEXT TO the page, so a blocking state must cover the page itself: a modal
// layer (Radix Dialog: the rest of the app is made inert and aria-hidden, focus stays inside, no way
// to dismiss it) with a heading, shown for `pending`, `revoked`, `account_blocked`, `key_changed`,
// `desktop_refused` and `policy`. `login_required`, `web_outdated` and `desktop_outdated` are handled by
// the layout and never reach this slot. There is deliberately no "trust the new key" control: a changed
// key is accepted only through `/desktops/:id/verify#fp=` (R-14).
import { Link } from "@tanstack/react-router";
import { useEffect, type ReactNode } from "react";
import { useLinkConnection, useLinkDesktopId, useLinkState } from "@/link";
import { AuthLayout } from "@/ui/AuthLayout";
import { Button, buttonVariants } from "@/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/ui/dialog";
import { useDelayed } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { setBlockingLayerShown } from "./blockingLayer";
import { blockingScreen, type BlockingScreen } from "./linkStateModel";
import { safeName } from "./format";
import { FingerprintLines } from "./parts";
import { useLazyGrant } from "./useLazyGrant";

/** Remounts per desktop so the grant bookkeeping of one desktop never leaks into the next. */
export function LinkStateScreens() {
  const desktopId = useLinkDesktopId();
  return <LinkStateLayer key={desktopId} desktopId={desktopId} />;
}

function LinkStateLayer({ desktopId }: { desktopId: string }) {
  const grant = useLazyGrant(desktopId);
  const status = useLinkState((state) => state.status);
  const blocked = useLinkState((state) => state.blocked);
  const screen = blockingScreen({ status, blocked }, grant.phase, grant.raceOpen);
  // "Requesting access..." only shows when the request takes longer than 250 ms (no flash on a fast answer).
  const requestingShown = useDelayed(screen === "requesting");
  const visible = screen !== null && (screen !== "requesting" || requestingShown);
  useEffect(() => {
    setBlockingLayerShown(desktopId, visible);
    return () => setBlockingLayerShown(desktopId, false);
  }, [desktopId, visible]);
  if (screen === null || !visible) return null;
  return <BlockingLayer screen={screen} desktopId={desktopId} onRetryGrant={grant.retry} />;
}

// The secondary actions use the filled `secondary` variant: the 8% outline stroke is nearly invisible on dark.
const BACK_TO_DESKTOPS = (
  <Link to="/settings/desktops" className={buttonVariants({ variant: "secondary", size: "lg", className: "w-full" })}>
    Go to desktops
  </Link>
);

function BlockingLayer({ screen, desktopId, onRetryGrant }: { screen: BlockingScreen; desktopId: string; onRetryGrant: () => void }) {
  const connection = useLinkConnection();
  const desktopName = useLinkState((state) => state.desktopName);
  const desktopFingerprint = useLinkState((state) => state.desktopFingerprint);
  const deviceFingerprint = useLinkState((state) => state.deviceFingerprint);
  const name = safeName(desktopName ?? "", "this desktop");

  let title: string;
  let body: ReactNode;
  let actions: ReactNode = BACK_TO_DESKTOPS;
  switch (screen) {
    case "requesting":
      title = "Requesting access...";
      body = <p>Asking for access for this device. This takes a moment.</p>;
      actions = null;
      break;
    case "pending":
      title = "Confirm this device on your desktop";
      body = (
        <>
          <p>
            {name} is showing a confirmation dialog. Compare the two fingerprints below with the ones it shows, and confirm only if both match. This page updates by itself once you confirm.
          </p>
          <FingerprintBlock label="Desktop fingerprint" hex={desktopFingerprint} />
          <FingerprintBlock label="This device's fingerprint" hex={deviceFingerprint} />
          <Notice>
            <span className="flex items-center gap-2">
              <Icon name="spinner" size={16} className="shrink-0 animate-spin motion-reduce:animate-none" />
              <span>Waiting for confirmation on {name}. Keep Monoptah open on the desktop.</span>
            </span>
          </Notice>
        </>
      );
      break;
    case "revoked":
      title = "Access to this desktop was revoked";
      body = (
        <>
          <p>This device can no longer use {name}. A revoked device comes back only as a new device.</p>
          <p>
            To get access again, forget this device in All desktops (under "Browsers with desktop access"). That removes this device's access on every desktop. Then open the desktop again and confirm it on the desktop.
          </p>
          <p>
            Before you confirm, compare the fingerprints on the confirmation screen with the ones in the dialog on your desktop. They must match exactly. If they differ, do not confirm.
          </p>
        </>
      );
      break;
    case "account_blocked":
      title = "This account is blocked";
      body = <p>This account cannot use Monoptah right now. Ask the person who runs this Monoptah server to unblock it.</p>;
      break;
    case "key_changed":
      title = "Desktop key changed";
      body = (
        <>
          <p>The key of {name} is not the one this device trusted before. This can happen after a reinstall, but it can also be someone pretending to be your desktop. Do not continue until you have checked.</p>
          <FingerprintBlock label="Pinned desktop fingerprint" hex={desktopFingerprint} />
          <p>
            On your desktop, open Settings, then Web app, then "Verify on another device". Open that link, or scan its QR code, on this device. It confirms the new key only when it matches.
          </p>
        </>
      );
      actions = (
        <Link to="/settings/desktops/$id" params={{ id: desktopId }} className={buttonVariants({ variant: "secondary", size: "lg", className: "w-full" })}>
          Manage this desktop
        </Link>
      );
      break;
    case "desktop_refused":
      title = "This desktop did not accept this device";
      body = <p>The desktop closed the connection before it knew this device. It may be busy or restarting. You can try again.</p>;
      actions = (
        <>
          <Button type="button" size="lg" className="w-full" onClick={() => connection.reconnect()}>
            Try again
          </Button>
          {BACK_TO_DESKTOPS}
        </>
      );
      break;
    case "grant_failed":
      title = "Could not request access";
      body = <Notice variant="error">Access for this device could not be requested. Check your connection and try again.</Notice>;
      actions = (
        <>
          <Button type="button" size="lg" className="w-full" onClick={onRetryGrant}>
            Try again
          </Button>
          {BACK_TO_DESKTOPS}
        </>
      );
      break;
    case "policy":
      title = "This desktop closed the connection";
      body = <p>The desktop ended the connection on purpose. Check the desktop, then open it again from Desktops.</p>;
      break;
  }

  return (
    <Dialog open onOpenChange={() => undefined}>
      <DialogContent
        role="alertdialog"
        showClose={false}
        onEscapeKeyDown={(event) => event.preventDefault()}
        onPointerDownOutside={(event) => event.preventDefault()}
        onInteractOutside={(event) => event.preventDefault()}
        data-screen={screen}
        className="top-0 left-0 h-dvh max-h-none w-full max-w-none translate-x-0 translate-y-0 content-start overflow-y-auto rounded-none border-0 bg-background p-0 pt-[max(2.5rem,env(safe-area-inset-top))] pb-[calc(env(safe-area-inset-bottom)+2rem)] data-[state=open]:animate-[fade-in_180ms_var(--ease-out-cubic)]"
      >
        {/* The same column and card as the sign-in and pair screens that lead here (AuthLayout). */}
        <AuthLayout>
          <p className="text-label font-semibold text-content">Monoptah</p>
          <DialogTitle className="text-headline font-semibold wrap-anywhere">{title}</DialogTitle>
          <DialogDescription asChild>
            <div className="flex flex-col gap-4 text-body text-muted-text">{body}</div>
          </DialogDescription>
          {actions ? <div className="flex flex-col gap-2">{actions}</div> : null}
        </AuthLayout>
      </DialogContent>
    </Dialog>
  );
}

function FingerprintBlock({ label, hex }: { label: string; hex: string | null }) {
  return (
    <div className="flex flex-col gap-1 rounded-card bg-surface p-4">
      <span className="text-meta font-semibold text-content">{label}</span>
      {hex ? <FingerprintLines hex={hex} /> : <span className="text-small text-muted-text">Not known yet</span>}
    </div>
  );
}
