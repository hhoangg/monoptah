import { Switch as SwitchPrimitive } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "./cn";

// Switch (design-system.md section 8): the track is `brand` when on, `fill` when off. No shadow.
export function Switch({ className, ...props }: ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "relative inline-flex h-8 w-[52px] shrink-0 items-center rounded-pill border border-stroke transition-colors data-[state=checked]:bg-brand data-[state=unchecked]:bg-fill disabled:opacity-40",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="pointer-events-none block size-6 translate-x-0.5 rounded-pill bg-white transition-transform data-[state=checked]:translate-x-[22px]"
      />
    </SwitchPrimitive.Root>
  );
}
