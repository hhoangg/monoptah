// A context menu (right click, long press) styled like the app's dropdown menu (ui/dropdown-menu.tsx), for the Mono
// list (F-13) and message actions (F-71). The `ui/` kit has no context menu, so it is built here from radix.
//
// Placement: Radix measures against floating-ui, whose viewport boundary is the visual viewport (`getViewportRect` uses
// `visualViewport.width/height/offset*`), so the iOS keyboard already counts and `collisionPadding` plus the
// `available-height` cap are enough. `useAnchoredPlacement` is for popups that hang off their parent (the `/` menu) and
// does not fit a menu opened at the pointer.
import { ContextMenu as ContextMenuPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
import { VISIBLE_MARGIN_PX } from "@/ui/anchoredPlacement";
import { MENU_ITEM_ATTR, MENU_ROW, MENU_ROW_DESTRUCTIVE, MENU_SEPARATOR, MENU_SURFACE, MENU_SURFACE_ATTR } from "@/ui/menuSurface";
import { useOverlayOwnerAttr } from "@/ui/overlayOwner";

export const ContextMenu = ContextMenuPrimitive.Root;
export const ContextMenuTrigger = ContextMenuPrimitive.Trigger;

export function ContextMenuContent({
  className,
  collisionPadding = VISIBLE_MARGIN_PX,
  ...props
}: ComponentProps<typeof ContextMenuPrimitive.Content>) {
  const owner = useOverlayOwnerAttr();
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        {...owner}
        {...MENU_SURFACE_ATTR}
        collisionPadding={collisionPadding}
        className={cn(`z-50 min-w-48 max-h-[var(--radix-context-menu-content-available-height)] overflow-y-auto ${MENU_SURFACE}`, className)}
        {...props}
      />
    </ContextMenuPrimitive.Portal>
  );
}

export function ContextMenuItem({ className, destructive = false, ...props }: ComponentProps<typeof ContextMenuPrimitive.Item> & { destructive?: boolean }) {
  return <ContextMenuPrimitive.Item {...MENU_ITEM_ATTR} className={cn(MENU_ROW, destructive && MENU_ROW_DESTRUCTIVE, className)} {...props} />;
}

export function ContextMenuSeparator({ className, ...props }: ComponentProps<typeof ContextMenuPrimitive.Separator>) {
  return <ContextMenuPrimitive.Separator className={cn(MENU_SEPARATOR, className)} {...props} />;
}
