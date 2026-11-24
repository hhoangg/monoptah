import type { ReactNode } from "react";
import { Button } from "./button";
import { DelayedSpinner } from "./DelayedSpinner";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./dialog";
import { Notice } from "./Notice";

export interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  /** Destructive confirm uses the `deletions` color. */
  destructive?: boolean;
  /** Disables both buttons and shows a spinner (after 250 ms) while the action runs. */
  busy?: boolean;
  /** An error from the last attempt, shown inside the dialog. */
  error?: string | null;
  /** A soft, non-error message (for example a closed passkey prompt). */
  notice?: string | null;
  /** Keeps the confirm button off (for example while what the dialog offers is still loading). */
  confirmDisabled?: boolean;
  onConfirm: () => void;
  /** An extra action between Cancel and the confirm button (for example "Remove anyway"). */
  secondary?: { label: string; onClick: () => void };
}

/**
 * Confirmation (design-system.md section 11: confirmations are a centered `Dialog`, radius 20, on
 * every breakpoint). The dialog cannot be dismissed while `busy`. The caller closes it on success.
 */
export function ConfirmDialog({ open, onOpenChange, title, description, confirmLabel, destructive = false, busy = false, error, notice, confirmDisabled = false, onConfirm, secondary }: ConfirmDialogProps) {
  return (
    <Dialog open={open} onOpenChange={(next) => (busy ? undefined : onOpenChange(next))}>
      <DialogContent showClose={false} onEscapeKeyDown={(event) => busy && event.preventDefault()} onInteractOutside={(event) => busy && event.preventDefault()}>
        <DialogHeader className="pr-0">
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        {error ? <Notice variant="error">{error}</Notice> : null}
        {notice ? <Notice variant="info">{notice}</Notice> : null}
        <DialogFooter>
          <Button type="button" variant="ghost" disabled={busy} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          {secondary ? (
            <Button type="button" variant="outline" disabled={busy} onClick={secondary.onClick}>
              {secondary.label}
            </Button>
          ) : null}
          <Button type="button" variant={destructive ? "destructive" : "default"} disabled={busy || confirmDisabled} aria-busy={busy} onClick={onConfirm}>
            {confirmLabel}
            {busy ? <DelayedSpinner label="Working" /> : null}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
