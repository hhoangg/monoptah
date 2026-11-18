import { Dialog as DialogPrimitive } from "radix-ui";
import { useRef, type ComponentProps } from "react";
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
  onOpenAutoFocus,
  onCloseAutoFocus,
  ...props
}: ComponentProps<typeof DialogPrimitive.Content> & { showClose?: boolean }) {
  // Radix returns focus only to a `DialogTrigger`. Dialogs here are mostly controlled (opened from state), so without
  // this focus would fall to the page body when they close and a keyboard user would lose their place.
  const opener = useRef<HTMLElement | null>(null);
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        onOpenAutoFocus={(event) => {
          opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
          onOpenAutoFocus?.(event);
        }}
        onCloseAutoFocus={(event) => {
          onCloseAutoFocus?.(event);
          const target = opener.current;
          opener.current = null;
          // A caller that prevented the default handles focus itself. Radix then runs its own handler, which focuses
          // the trigger when there is one (after this, so it wins) and does nothing otherwise.
          if (event.defaultPrevented || target === null || !target.isConnected) return;
          target.focus();
        }}
        className={cn(
          "fixed top-1/2 left-1/2 z-50 grid w-[calc(100%-2.5rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 max-h-[calc(100dvh-2rem-env(safe-area-inset-top)-env(safe-area-inset-bottom))] gap-4 overflow-y-auto overscroll-contain rounded-dialog border border-stroke bg-sheet p-5 text-content data-[state=open]:animate-[dialog-in_280ms_var(--ease-out-cubic)] data-[state=closed]:animate-[fade-out_180ms_var(--ease-out-cubic)]",
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
  return <div data-slot="dialog-footer" className={cn("flex flex-col-reverse gap-2 max-sm:[&>*]:w-full sm:flex-row sm:flex-wrap sm:justify-end", className)} {...props} />;
}

export function DialogTitle({ className, ...props }: ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title data-slot="dialog-title" className={cn("text-title font-semibold", className)} {...props} />;
}

export function DialogDescription({ className, ...props }: ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description data-slot="dialog-description" className={cn("text-small text-muted-text", className)} {...props} />
  );
}
