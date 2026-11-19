// The confirm dialog of the cloud UI. It is `ConfirmDialog` plus two things the cloud actions need:
// a confirm that can be switched off (permission or capability lost while the dialog is open, re-checked by
// the caller at render and again at confirm time), and a wrapper that keeps clicks inside the dialog from
// reaching a React ancestor. React events bubble through portals, so a dialog opened from a session card
// would otherwise click the card's `<Link>` and navigate away.
import type { ReactNode } from "react";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Notice } from "@/ui/Notice";

/** Stops a click from reaching the card link. Only `click`: Radix dismisses on `pointerdown`, which stays untouched. */
export function ContainClicks({ children }: { children: ReactNode }) {
  return (
    <div style={{ display: "contents" }} onClick={(event) => event.stopPropagation()}>
      {children}
    </div>
  );
}

export interface CloudConfirmProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  destructive?: boolean;
  busy?: boolean;
  /** Turns the confirm off (for example a device without control). The dialog can still be cancelled. */
  confirmDisabled?: boolean;
  error?: string | null;
  /** A soft message under the description, for example the read-only note. */
  notice?: string | null;
  onConfirm: () => void;
  children?: ReactNode;
}

export function CloudConfirm({ open, onOpenChange, title, description, confirmLabel, destructive = false, busy = false, confirmDisabled = false, error, notice, onConfirm, children }: CloudConfirmProps) {
  return (
    <ContainClicks>
      <Dialog open={open} onOpenChange={(next) => (busy ? undefined : onOpenChange(next))}>
        <DialogContent showClose={false} onEscapeKeyDown={(event) => busy && event.preventDefault()} onInteractOutside={(event) => busy && event.preventDefault()}>
          <DialogHeader className="pr-0">
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </DialogHeader>
          {children}
          {error ? <Notice variant="error">{error}</Notice> : null}
          {notice ? <Notice variant="info">{notice}</Notice> : null}
          <DialogFooter>
            <Button type="button" variant="ghost" disabled={busy} onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              type="button"
              variant={destructive ? "destructive" : "default"}
              disabled={busy || confirmDisabled}
              aria-busy={busy}
              onClick={() => {
                // Re-checked at confirm time: a disabled state that changed after the last render still stops the request.
                if (busy || confirmDisabled) return;
                onConfirm();
              }}
            >
              {confirmLabel}
              {busy ? <DelayedSpinner label="Working" /> : null}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ContainClicks>
  );
}
