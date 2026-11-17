import { Dialog as DialogPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
import { Icon } from "./Icon";

// Dialog (design-system.md section 8): bg sheet, radius 20, padding 20, gap 16, scrim .4, no shadow.
export const Dialog = DialogPrimitive.Root;
export const DialogTrigger = DialogPrimitive.Trigger;
export const DialogPortal = DialogPrimitive.Portal;
export const DialogClose = DialogPrimitive.Close;

export function DialogOverlay({ className, ...props }: ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 z-50 bg-[var(--scrim)] data-[state=open]:animate-[fade-in_180ms_var(--ease-out-cubic)] data-[state=closed]:animate-[fade-out_180ms_var(--ease-out-cubic)]",
        className,
      )}
      {...props}
    />
  );
}

export function DialogContent({
  className,
  children,
  showClose = true,
  ...props
}: ComponentProps<typeof DialogPrimitive.Content> & { showClose?: boolean }) {
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          "fixed top-1/2 left-1/2 z-50 grid w-[calc(100%-2.5rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 gap-4 rounded-dialog border border-stroke bg-sheet p-5 text-content data-[state=open]:animate-[dialog-in_280ms_var(--ease-out-cubic)] data-[state=closed]:animate-[fade-out_180ms_var(--ease-out-cubic)]",
          className,
        )}
        {...props}
      >
        {children}
        {showClose && (
          <DialogPrimitive.Close
            aria-label="Close"
            className="absolute top-3 right-3 inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
          >
            <Icon name="close" size={20} />
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPortal>
  );
}

export function DialogHeader({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="dialog-header" className={cn("grid gap-1.5 pr-10", className)} {...props} />;
}

export function DialogFooter({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="dialog-footer" className={cn("flex justify-end gap-2", className)} {...props} />;
}

export function DialogTitle({ className, ...props }: ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title data-slot="dialog-title" className={cn("text-title font-semibold", className)} {...props} />;
}

export function DialogDescription({ className, ...props }: ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description data-slot="dialog-description" className={cn("text-small text-muted-text", className)} {...props} />
  );
}
