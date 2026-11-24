import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
import { VISIBLE_MARGIN_PX } from "./anchoredPlacement";
import { MENU_ITEM_ATTR, MENU_ROW, MENU_ROW_DESTRUCTIVE, MENU_SEPARATOR, MENU_SURFACE, MENU_SURFACE_ATTR } from "./menuSurface";
import { useOverlayOwnerAttr } from "./overlayOwner";

// Dropdown menu for `pointer: fine` (design-system.md section 8): bg sheet, menu-card radius,
// rows min 44 (36 on pointer: fine), highlight = selection, destructive = deletions, no shadow. The surface and the
// concentric item radius are shared with every other popup (`menuSurface.ts`). The content never grows past the room
// Radix measures (`available-height`) and scrolls inside instead.
export const DropdownMenu = DropdownMenuPrimitive.Root;
export const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger;
export const DropdownMenuGroup = DropdownMenuPrimitive.Group;
export const DropdownMenuPortal = DropdownMenuPrimitive.Portal;

export function DropdownMenuContent({
  className,
  sideOffset = 6,
  collisionPadding = VISIBLE_MARGIN_PX,
  ...props
}: ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  const owner = useOverlayOwnerAttr();
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        {...owner}
        {...MENU_SURFACE_ATTR}
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        collisionPadding={collisionPadding}
        className={cn(
          `z-50 min-w-48 max-h-[var(--radix-dropdown-menu-content-available-height)] overflow-y-auto ${MENU_SURFACE} data-[state=open]:animate-[fade-in_180ms_var(--ease-out-cubic)]`,
          className,
        )}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

export function DropdownMenuItem({
  className,
  destructive = false,
  ...props
}: ComponentProps<typeof DropdownMenuPrimitive.Item> & { destructive?: boolean }) {
  return (
    <DropdownMenuPrimitive.Item
      {...MENU_ITEM_ATTR}
      data-slot="dropdown-menu-item"
      className={cn(
        MENU_ROW,
        destructive && MENU_ROW_DESTRUCTIVE,
        className,
      )}
      {...props}
    />
  );
}

export function DropdownMenuLabel({ className, ...props }: ComponentProps<typeof DropdownMenuPrimitive.Label>) {
  return (
    <DropdownMenuPrimitive.Label
      data-slot="dropdown-menu-label"
      className={cn("px-3 py-1.5 text-meta font-medium text-muted-text", className)}
      {...props}
    />
  );
}

export function DropdownMenuSeparator({ className, ...props }: ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return (
    <DropdownMenuPrimitive.Separator
      data-slot="dropdown-menu-separator"
      className={cn(MENU_SEPARATOR, className)}
      {...props}
    />
  );
}
