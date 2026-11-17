import type { ComponentProps } from "react";
import { cn } from "./cn";

// Text input (design-system.md section 8): bg fill, radius 12, 17px text (no iOS zoom), 1px stroke,
// focus is the global 2px accent outline, never a ring shadow.
export function Input({ className, type, ...props }: ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "min-h-11 w-full min-w-0 rounded-row border border-stroke bg-fill px-4 py-2 text-row text-content placeholder:text-muted-text disabled:pointer-events-none disabled:opacity-40 pointer-fine:min-h-9",
        className,
      )}
      {...props}
    />
  );
}
