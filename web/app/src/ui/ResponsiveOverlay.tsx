import type { ReactNode } from "react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./dialog";
import { Drawer, DrawerContent, DrawerDescription, DrawerHeader, DrawerTitle } from "./drawer";
import { useMediaQuery } from "./useMediaQuery";

/** Width from which an overlay is a centered Dialog instead of a bottom sheet (design-system.md section 11). */
export const OVERLAY_DIALOG_QUERY = "(min-width: 640px)";

export interface ResponsiveOverlayProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  /** Scrolls when taller than the viewport. */
  children: ReactNode;
  /** Pinned under the body (buttons). */
  footer?: ReactNode;
}

/**
 * One overlay for content-heavy panels: a vaul `Drawer` under 640px, a `Dialog` from 640px
 * (design-system.md section 11). The title is the accessible name on both.
 */
export function ResponsiveOverlay({ open, onOpenChange, title, description, children, footer }: ResponsiveOverlayProps) {
  const wide = useMediaQuery(OVERLAY_DIALOG_QUERY, true);
  if (wide) {
    return (
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-h-[85dvh] max-w-xl grid-rows-[auto_minmax(0,1fr)_auto]" {...(description ? {} : { "aria-describedby": undefined })}>
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            {description ? <DialogDescription>{description}</DialogDescription> : null}
          </DialogHeader>
          <div className="-m-1 min-h-0 overflow-y-auto p-1">{children}</div>
          {footer ? <div className="flex flex-wrap justify-end gap-2">{footer}</div> : null}
        </DialogContent>
      </Dialog>
    );
  }
  return (
    <Drawer open={open} onOpenChange={onOpenChange}>
      <DrawerContent {...(description ? {} : { "aria-describedby": undefined })}>
        <DrawerHeader>
          <DrawerTitle>{title}</DrawerTitle>
          {description ? <DrawerDescription>{description}</DrawerDescription> : null}
        </DrawerHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
          {/* 4 + 16 = 20px, the same inset as the header and footer, with room for the focus ring. */}
          <div className="px-1">{children}</div>
        </div>
        {footer ? <div className="flex flex-col-reverse gap-2 px-5 pt-2">{footer}</div> : null}
      </DrawerContent>
    </Drawer>
  );
}
