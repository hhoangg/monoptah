import type { ComponentProps } from "react";
// vaul injects its stylesheet with a <style> element at runtime, which the production CSP (style-src 'self')
// blocks: sheets lose drag, animation and the backdrop extension. Its CSS is bundled from styles/vaul.css instead.
import "@/styles/vaul.css";
import { Drawer as DrawerPrimitive } from "vaul";
import { cn } from "@/lib/utils";

// Sheet (design-system.md section 8 maps ModalSheet and the touch ChoiceMenu to vaul Drawer):
// bg sheet, top radius 32, scrim .4, grabber 36x5 radius 2.5 in `faint`, max height 92dvh,
// bottom padding = safe-area inset + 16. vaul uses Radix Dialog, so it traps focus and sets aria-modal.
export const Drawer = DrawerPrimitive.Root;
export const DrawerTrigger = DrawerPrimitive.Trigger;
export const DrawerPortal = DrawerPrimitive.Portal;
export const DrawerClose = DrawerPrimitive.Close;

export function DrawerOverlay({ className, ...props }: ComponentProps<typeof DrawerPrimitive.Overlay>) {
  return (
    <DrawerPrimitive.Overlay
      data-slot="drawer-overlay"
      className={cn("fixed inset-0 z-50 bg-[var(--scrim)]", className)}
      {...props}
    />
  );
}

export function DrawerContent({ className, children, ...props }: ComponentProps<typeof DrawerPrimitive.Content>) {
  return (
    <DrawerPortal>
      <DrawerOverlay />
      <DrawerPrimitive.Content
        data-slot="drawer-content"
        className={cn(
          "fixed inset-x-0 bottom-0 z-50 mt-24 flex max-h-[92dvh] flex-col rounded-t-sheet border border-b-0 border-stroke bg-sheet pb-[calc(env(safe-area-inset-bottom)+1rem)] text-content",
          className,
        )}
        {...props}
      >
        <div aria-hidden="true" className="mx-auto mt-2 h-[5px] w-9 shrink-0 rounded-[2.5px] bg-faint" />
        {children}
      </DrawerPrimitive.Content>
    </DrawerPortal>
  );
}

export function DrawerHeader({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="drawer-header" className={cn("grid min-h-14 gap-1 px-5 pt-3", className)} {...props} />;
}

export function DrawerTitle({ className, ...props }: ComponentProps<typeof DrawerPrimitive.Title>) {
  return <DrawerPrimitive.Title data-slot="drawer-title" className={cn("text-title font-semibold", className)} {...props} />;
}

export function DrawerDescription({ className, ...props }: ComponentProps<typeof DrawerPrimitive.Description>) {
  return (
    <DrawerPrimitive.Description data-slot="drawer-description" className={cn("text-small text-muted-text", className)} {...props} />
  );
}
