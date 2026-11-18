// A context menu (right click, long press) styled like the app's dropdown menu (ui/dropdown-menu.tsx), for the Mono
// list (F-13) and message actions (F-71). The `ui/` kit has no context menu, so it is built here from radix.
import { ContextMenu as ContextMenuPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export const ContextMenu = ContextMenuPrimitive.Root;
export const ContextMenuTrigger = ContextMenuPrimitive.Trigger;

export function ContextMenuContent({ className, ...props }: ComponentProps<typeof ContextMenuPrimitive.Content>) {
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        className={cn("z-50 min-w-48 overflow-hidden rounded-card border border-stroke bg-popover p-1.5 text-popover-foreground", className)}
        {...props}
      />
    </ContextMenuPrimitive.Portal>
  );
}

export function ContextMenuItem({ className, destructive = false, ...props }: ComponentProps<typeof ContextMenuPrimitive.Item> & { destructive?: boolean }) {
  return (
    <ContextMenuPrimitive.Item
      className={cn(
        "flex min-h-11 cursor-default items-center gap-3 rounded-row px-3 text-row outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-40 data-[highlighted]:bg-selection pointer-fine:min-h-9",
        destructive && "text-deletions",
        className,
      )}
      {...props}
    />
  );
}

export function ContextMenuSeparator({ className, ...props }: ComponentProps<typeof ContextMenuPrimitive.Separator>) {
  return <ContextMenuPrimitive.Separator className={cn("mx-3 my-1 h-px bg-stroke", className)} {...props} />;
}
