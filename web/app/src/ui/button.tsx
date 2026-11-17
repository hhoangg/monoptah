import { cva, type VariantProps } from "class-variance-authority";
import { Slot as SlotPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { haptic } from "@/lib/haptics";
import { cn } from "@/lib/utils";

// shadcn Button restyled to design-system.md section 8: the primary action is the inverse pair
// (bg primary = inverseSurface, text inverse), never accent-colored. No shadows, radii from the
// scale, >= 44px on touch and 36px on pointer: fine, visible focus comes from the global outline.
export const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap text-small font-semibold outline-offset-2 transition-[opacity,background-color] duration-150 select-none active:opacity-60 disabled:pointer-events-none disabled:opacity-40 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground",
        secondary: "bg-secondary text-secondary-foreground hover:bg-selection",
        outline: "border border-stroke bg-transparent text-content hover:bg-selection",
        ghost: "bg-transparent text-content hover:bg-selection",
        link: "bg-transparent text-brand-text hover:underline",
        destructive: "bg-destructive text-inverse",
        /** Round icon button over content: bg fill, icon turns brand when `data-on`. */
        circle: "rounded-pill bg-fill text-content hover:bg-selection data-[on=true]:text-brand",
      },
      size: {
        default: "min-h-11 px-4 py-2 pointer-fine:min-h-9 rounded-row",
        sm: "min-h-9 px-3 rounded-row",
        /** Big call to action: radius 32. */
        lg: "min-h-14 px-6 text-body rounded-sheet",
        /** Send button and other pills: radius 999. */
        pill: "min-h-11 px-5 pointer-fine:min-h-9 rounded-pill",
        /** Circle button 44, 36 in the composer and on pointer: fine. */
        circle: "size-11 pointer-fine:size-9 p-0",
        "circle-sm": "size-9 p-0",
      },
    },
    compoundVariants: [{ variant: "circle", class: "rounded-pill" }],
    defaultVariants: { variant: "default", size: "default" },
  },
);

export interface ButtonProps extends ComponentProps<"button">, VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export function Button({ className, variant, size, asChild = false, onPointerDown, ...props }: ButtonProps) {
  const Comp = asChild ? SlotPrimitive.Root : "button";
  const isCircle = variant === "circle";
  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size: size ?? (isCircle ? "circle" : undefined) }), className)}
      onPointerDown={(event: React.PointerEvent<HTMLButtonElement>) => {
        if (isCircle) haptic("light");
        onPointerDown?.(event);
      }}
      {...props}
    />
  );
}
