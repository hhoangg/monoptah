import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// tailwind-merge does not know the design-system type scale (`text-body`, `text-small`, ...) or the
// radius names (`rounded-card`, ...). Without this it reads `text-body` as a text COLOR, so
// `cn("text-primary-foreground", "text-body")` silently drops the color (and `text-small` before
// a color utility drops the font size). Names mirror styles/tailwind.css (@theme).
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      text: ["display", "headline", "title", "h", "row", "body", "label", "small", "meta", "caption", "micro"],
      radius: ["pill", "card", "row", "sheet", "menu-card", "dialog"],
    },
  },
});

/** Merges class names; later Tailwind utilities win over earlier ones. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
