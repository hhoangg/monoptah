import { motion, type HTMLMotionProps } from "motion/react";
import { haptic } from "@/lib/haptics";
import { cn } from "@/lib/utils";
import { springs } from "./motion";

const INTERACTIVE = 'button:not(:disabled), a[href], [role="button"]';

export interface GlassProps extends HTMLMotionProps<"div"> {
  /** Replaces the wash color (`--glass-tint`). Pass a CSS color or `var(--token)`. */
  tint?: string;
  /** Press feedback: scale 1.05 with the press spring, a light haptic, and a 4px hit slop. */
  pressable?: boolean;
}

/**
 * Glass surface (design-system.md 4.1): blurred wash of `surface` plus a gradient hairline.
 * Used by floating controls, the composer and pickers. The CSS lives in styles/glass.css.
 */
export function Glass({ tint, pressable = false, className, style, onPointerDown, onClick, ...rest }: GlassProps) {
  return (
    <motion.div
      {...rest}
      data-pressable={pressable ? "true" : undefined}
      className={cn("glass", className)}
      style={tint ? { ...style, ["--glass-tint" as string]: tint } : style}
      whileTap={pressable ? { scale: 1.05 } : undefined}
      transition={pressable ? springs.press : rest.transition}
      onClick={(event) => {
        onClick?.(event);
        // A press in the hit slop lands on the surface itself: hand it to the control inside.
        if (pressable && !event.defaultPrevented && event.target === event.currentTarget) {
          event.currentTarget.querySelector<HTMLElement>(INTERACTIVE)?.click();
        }
      }}
      onPointerDown={(event) => {
        if (pressable) haptic("light");
        onPointerDown?.(event);
      }}
    />
  );
}
