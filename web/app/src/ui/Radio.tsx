import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

/** Native radio (design-system.md 3.3): 18x18, 1.5px border faint, checked draws a 5px content ring. Wrap it in a >= 44px row. */
export function Radio({ className, ...input }: Omit<ComponentProps<"input">, "type">) {
  return (
    <input
      type="radio"
      className={cn("size-[18px] shrink-0 cursor-pointer appearance-none rounded-pill border-[1.5px] border-faint bg-transparent checked:border-[5px] checked:border-content", className)}
      {...input}
    />
  );
}
