import { Tooltip as TooltipPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "./cn";

// Tooltip (web only, design-system.md section 8): bg sheet, 1px stroke, radius 8, 13px. Pair it
// with `pointer: fine` at the call site; touch devices have no hover.
export const TooltipProvider = TooltipPrimitive.Provider;
export const Tooltip = TooltipPrimitive.Root;
export const TooltipTrigger = TooltipPrimitive.Trigger;

export function TooltipContent({ className, sideOffset = 6, ...props }: ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        data-slot="tooltip-content"
        sideOffset={sideOffset}
        className={cn("z-50 rounded-[8px] border border-stroke bg-sheet px-2.5 py-1.5 text-meta text-content", className)}
        {...props}
      />
    </TooltipPrimitive.Portal>
  );
}
